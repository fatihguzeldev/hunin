//! The wire fields follow Google's canonical perftools.profiles profile.proto.
use super::sample;
use anyhow::{Context, Result, bail};
use prost::Message;
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Clone, PartialEq, Message)]
struct Profile {
    #[prost(message, repeated, tag = "1")]
    sample_type: Vec<ValueType>,
    #[prost(message, repeated, tag = "2")]
    sample: Vec<Sample>,
    #[prost(message, repeated, tag = "3")]
    mapping: Vec<Mapping>,
    #[prost(message, repeated, tag = "4")]
    location: Vec<Location>,
    #[prost(message, repeated, tag = "5")]
    function: Vec<Function>,
    #[prost(string, repeated, tag = "6")]
    string_table: Vec<String>,
    #[prost(int64, tag = "9")]
    time_nanos: i64,
    #[prost(int64, tag = "10")]
    duration_nanos: i64,
    #[prost(message, optional, tag = "11")]
    period_type: Option<ValueType>,
    #[prost(int64, tag = "12")]
    period: i64,
    #[prost(int64, tag = "14")]
    default_sample_type: i64,
}
#[derive(Clone, PartialEq, Message)]
struct ValueType {
    #[prost(int64, tag = "1")]
    r#type: i64,
    #[prost(int64, tag = "2")]
    unit: i64,
}
#[derive(Clone, PartialEq, Message)]
struct Sample {
    #[prost(uint64, repeated, tag = "1")]
    location_id: Vec<u64>,
    #[prost(int64, repeated, tag = "2")]
    value: Vec<i64>,
    #[prost(message, repeated, tag = "3")]
    label: Vec<Label>,
}
#[derive(Clone, PartialEq, Message)]
struct Label {
    #[prost(int64, tag = "1")]
    key: i64,
    #[prost(int64, tag = "2")]
    str: i64,
    #[prost(int64, tag = "3")]
    num: i64,
    #[prost(int64, tag = "4")]
    num_unit: i64,
}
#[derive(Clone, PartialEq, Message)]
struct Mapping {
    #[prost(uint64, tag = "1")]
    id: u64,
    #[prost(int64, tag = "5")]
    filename: i64,
    #[prost(int64, tag = "6")]
    build_id: i64,
}
#[derive(Clone, PartialEq, Message)]
struct Location {
    #[prost(uint64, tag = "1")]
    id: u64,
    #[prost(uint64, tag = "2")]
    mapping_id: u64,
    #[prost(uint64, tag = "3")]
    address: u64,
    #[prost(message, repeated, tag = "4")]
    line: Vec<Line>,
}
#[derive(Clone, PartialEq, Message)]
struct Line {
    #[prost(uint64, tag = "1")]
    function_id: u64,
    #[prost(int64, tag = "2")]
    line: i64,
    #[prost(int64, tag = "3")]
    column: i64,
}
#[derive(Clone, PartialEq, Message)]
struct Function {
    #[prost(uint64, tag = "1")]
    id: u64,
    #[prost(int64, tag = "2")]
    name: i64,
    #[prost(int64, tag = "3")]
    system_name: i64,
    #[prost(int64, tag = "4")]
    filename: i64,
    #[prost(int64, tag = "5")]
    start_line: i64,
}

pub(super) fn decode(bytes: &[u8], run_id: &str, key: &str) -> Result<(Vec<Value>, Vec<String>)> {
    let p = Profile::decode(bytes).context("decode pprof protobuf")?;
    if p.string_table.first().map(String::as_str) != Some("") {
        bail!("pprof string table must begin with empty string");
    }
    let text = |i: i64| -> Result<&str> {
        if i < 0 {
            bail!("negative pprof string index");
        }
        p.string_table
            .get(i as usize)
            .map(String::as_str)
            .context("pprof references missing string")
    };
    let selected = if p.default_sample_type != 0 {
        p.sample_type
            .iter()
            .position(|v| v.r#type == p.default_sample_type)
            .context("pprof default sample type missing")?
    } else {
        p.sample_type
            .len()
            .checked_sub(1)
            .context("pprof sample types missing")?
    };
    let kind = &p.sample_type[selected];
    let metric = text(kind.r#type)?;
    let source_unit = text(kind.unit)?;
    let (unit, factor) = match source_unit {
        "nanoseconds" => ("microseconds", 0.001),
        "microseconds" => ("microseconds", 1.0),
        "milliseconds" => ("microseconds", 1000.0),
        "seconds" => ("microseconds", 1_000_000.0),
        "bytes" => ("bytes", 1.0),
        "count" => (
            if metric == "samples" {
                "samples"
            } else {
                "count"
            },
            1.0,
        ),
        other => bail!("unsupported pprof measurement unit {other}"),
    };
    let locations: HashMap<_, _> = p.location.iter().map(|v| (v.id, v)).collect();
    let functions: HashMap<_, _> = p.function.iter().map(|v| (v.id, v)).collect();
    let mappings: HashMap<_, _> = p.mapping.iter().map(|v| (v.id, v)).collect();
    if locations.len() != p.location.len()
        || functions.len() != p.function.len()
        || mappings.len() != p.mapping.len()
    {
        bail!("duplicate pprof identity");
    }
    let mut records = Vec::new();
    for entry in &p.sample {
        if entry.value.len() != p.sample_type.len() {
            bail!("pprof sample value/type lengths differ");
        }
        let weight = entry.value[selected] as f64 * factor;
        if weight <= 0.0 {
            continue;
        }
        let mut stack = Vec::new();
        for location_id in entry.location_id.iter().rev() {
            let location = locations
                .get(location_id)
                .context("pprof sample references missing location")?;
            let mapping = mappings.get(&location.mapping_id);
            let mut address =
                json!({"function":"[unresolved]","address":format!("0x{:x}",location.address)});
            if let Some(mapping) = mapping {
                let name = text(mapping.filename)?;
                if !name.is_empty() {
                    address["module"] = json!(name);
                }
                let code = text(mapping.build_id)?;
                if !code.is_empty() {
                    address["code_id"] = json!(code);
                }
            }
            if location.line.is_empty() {
                stack.push(address);
            } else {
                for line in location.line.iter().rev() {
                    let function = functions
                        .get(&line.function_id)
                        .context("pprof location references missing function")?;
                    let mut frame = address.clone();
                    let name = text(function.name)?;
                    frame["function"] = json!(if name.is_empty() { "[unnamed]" } else { name });
                    let filename = text(function.filename)?;
                    if !filename.is_empty() {
                        frame["file"] = json!(filename);
                    }
                    if line.line > 0 {
                        frame["line"] = json!(line.line);
                    }
                    if line.column > 0 {
                        frame["column"] = json!(line.column - 1);
                    }
                    stack.push(frame);
                }
            }
        }
        let mut labels = serde_json::Map::new();
        let mut tid = String::from("unknown");
        for label in &entry.label {
            let name = text(label.key)?;
            let value = if label.str != 0 {
                json!(text(label.str)?)
            } else {
                json!(label.num)
            };
            if name == "thread_id" {
                tid = value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string());
            }
            labels.insert(name.to_owned(), value);
        }
        let mut attributes = json!({"source_format":"pprof","metric":metric,"source_unit":source_unit,"temporal_resolution":"aggregate","profile_duration_us":p.duration_nanos.max(0)/1000,"labels":labels});
        if p.time_nanos > 0 {
            attributes["clock"] = json!("unix_epoch");
            attributes["clock_alignment"] = json!("profile_collection_epoch");
        } else {
            attributes["clock"] = json!("unknown");
            attributes["clock_alignment"] = json!("unmapped");
        }
        records.push(sample(
            run_id,
            key,
            records.len(),
            0,
            &tid,
            (p.time_nanos.max(0) / 1000) as u64,
            stack,
            weight,
            unit,
            attributes,
        ));
    }
    Ok((records,vec!["pprof weights are aggregated over the profile interval; collection time does not establish individual sample times or an OS PID".into()]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_cpu_units_and_rejects_broken_location_references() {
        let mut p = Profile {
            sample_type: vec![ValueType { r#type: 1, unit: 2 }],
            string_table: vec!["".into(), "cpu".into(), "nanoseconds".into()],
            sample: vec![Sample {
                location_id: vec![9],
                value: vec![1_000_000],
                label: vec![],
            }],
            ..Default::default()
        };
        assert!(decode(&p.encode_to_vec(), "r", "k").is_err());
        p.location.push(Location {
            id: 9,
            address: 12,
            ..Default::default()
        });
        let (r, _) = decode(&p.encode_to_vec(), "r", "k").unwrap();
        assert_eq!(r[0]["weight"], 1000.0);
        assert_eq!(r[0]["weight_unit"], "microseconds");
        assert_eq!(r[0]["attributes"]["clock_alignment"], "unmapped");
    }
}

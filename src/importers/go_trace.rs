use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::HashMap, fs, path::Path, process::Command};

pub(super) fn decode(path: &Path, run_id: &str, key: &str) -> Result<(Vec<Value>, Vec<String>)> {
    let metadata: Value = fs::read(
        path.parent()
            .unwrap_or_else(|| Path::new("."))
            .join("go-tool.json"),
    )
    .ok()
    .map(|b| serde_json::from_slice(&b))
    .transpose()?
    .unwrap_or(json!({}));
    let go = metadata["executable"]
        .as_str()
        .map(std::path::PathBuf::from)
        .or_else(|| crate::collection::tool("go"))
        .context(
            "Go execution trace import requires a compatible Go toolchain on PATH or HUNIN_GO",
        )?;
    let mut output = Command::new(&go)
        .args(["tool", "trace", "-d=parsed"])
        .arg(path)
        .output()?;
    if !output.status.success()
        && String::from_utf8_lossy(&output.stderr).contains("invalid debug mode")
    {
        output = Command::new(&go)
            .args(["tool", "trace", "-d=1"])
            .arg(path)
            .output()?;
    }
    if !output.status.success() {
        bail!(
            "Go execution trace decode failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let text =
        std::str::from_utf8(&output.stdout).context("Go trace decoder emitted non-UTF8 data")?;
    parse(text, run_id, key)
}

fn parse(text: &str, run_id: &str, key: &str) -> Result<(Vec<Value>, Vec<String>)> {
    let mut records = Vec::new();
    let mut anchor: Option<(i128, i128)> = None;
    let mut ranges: HashMap<(String, String), (u64, String, Value)> = HashMap::new();
    let mut current: Option<Value> = None;
    let mut pending_function = None;
    for line in text.lines() {
        if line.starts_with("M=") {
            if let Some(record) = current.take() {
                finish(record, &mut records, &mut ranges, run_id, key)?;
            }
            let mut fields = line.split_whitespace();
            let m = fields
                .next()
                .and_then(|f| f.strip_prefix("M="))
                .context("Go trace missing M")?;
            let p = fields
                .next()
                .and_then(|f| f.strip_prefix("P="))
                .context("Go trace missing P")?;
            let g = fields
                .next()
                .and_then(|f| f.strip_prefix("G="))
                .context("Go trace missing G")?;
            let kind = fields.next().context("Go trace missing event kind")?;
            let time = field(line, "Time")
                .context("Go trace event time missing")?
                .parse::<i128>()?;
            if kind == "Sync" {
                if let (Some(trace), Some(wall)) = (field(line, "Trace"), field(line, "Wall")) {
                    let wall =
                        chrono::DateTime::parse_from_rfc3339(wall)?.timestamp_micros() as i128;
                    anchor = Some((trace.parse()?, wall));
                }
            }
            let mapped = anchor.map(|(trace, wall)| wall + (time - trace) / 1000);
            let timestamp = mapped.unwrap_or(time / 1000);
            if !(0..=9_007_199_254_740_991).contains(&timestamp) {
                bail!("Go trace timestamp outside safe microsecond range");
            }
            let mut attrs = json!({"source_format":"go-trace","go_m_id":m,"go_p_id":p,"goroutine_id":g,"raw_event":line,
                "clock":if mapped.is_some(){"unix_epoch"}else{"go_trace"},"clock_alignment":if mapped.is_some(){"go_clock_snapshot"}else{"unmapped"}});
            for name in ["Name", "Scope", "Reason", "GoID", "ProcID"] {
                if let Some(value) = field(line, name) {
                    attrs[name] = json!(value);
                }
            }
            let name = field(line, "Name")
                .map(|name| format!("go.{kind}: {name}"))
                .unwrap_or_else(|| format!("go.{kind}"));
            let index = records.len();
            let mut record = json!({"schema_version":1,"kind":"event","id":format!("profile:{key}:{index}"),"run_id":run_id,"producer_id":format!("profile:{key}:go"),"pid":0,"tid":m,"timestamp_us":timestamp as u64,"basis":"observed","name":name,"attributes":attrs});
            if kind == "Metric" {
                if let Some((value, unit)) = metric(line) {
                    record["kind"] = json!("metric");
                    record["value"] = json!(value);
                    record["unit"] = json!(unit);
                    record["scope"] = json!("process");
                }
            }
            current = Some(record);
            pending_function = None;
        } else if line.starts_with('\t') {
            if let Some((function, address)) = line.trim().split_once(" @ ") {
                pending_function = Some(json!({"function":function,"address":address}));
            } else if let Some(mut frame) = pending_function.take() {
                if let Some((file, line_number)) = line.trim().rsplit_once(':') {
                    frame["file"] = json!(file);
                    if let Ok(number) = line_number.parse::<u64>() {
                        if number > 0 {
                            frame["line"] = json!(number);
                        }
                    }
                }
                if let Some(record) = current.as_mut() {
                    if record.get("stack").is_none() {
                        record["stack"] = json!([]);
                    }
                    record["stack"].as_array_mut().unwrap().push(frame);
                }
            }
        }
    }
    if let Some(record) = current {
        finish(record, &mut records, &mut ranges, run_id, key)?;
    }
    if records.is_empty() {
        bail!("Go trace contains no parsed events");
    }
    let mut diagnostics=vec!["Go runtime trace M/G identities are retained separately from unknown OS PID; temporal overlap creates no causal links".into()];
    if anchor.is_none() {
        diagnostics.push(
            "Go trace contains no wall-clock snapshot; runtime event clock remains unmapped".into(),
        );
    }
    if !ranges.is_empty() {
        diagnostics.push(format!(
            "{} Go ranges remain open at recording end",
            ranges.len()
        ));
    }
    Ok((records, diagnostics))
}

fn finish(
    mut record: Value,
    records: &mut Vec<Value>,
    ranges: &mut HashMap<(String, String), (u64, String, Value)>,
    run_id: &str,
    key: &str,
) -> Result<()> {
    if let Some(stack) = record.get_mut("stack").and_then(Value::as_array_mut) {
        stack.reverse();
    }
    let raw = record["attributes"]["raw_event"].as_str().unwrap_or("");
    let kind = raw.split_whitespace().nth(3).unwrap_or("");
    let timestamp = record["timestamp_us"].as_u64().unwrap();
    let name = field(raw, "Name").unwrap_or("").to_owned();
    let scope = field(raw, "Scope").unwrap_or("").to_owned();
    if kind == "RangeBegin" {
        ranges.insert(
            (name, scope),
            (
                timestamp,
                record["id"].as_str().unwrap().to_owned(),
                record.clone(),
            ),
        );
    } else if kind == "RangeEnd" {
        if let Some((start, begin, original)) = ranges.remove(&(name.clone(), scope)) {
            if timestamp < start {
                bail!("Go range ends before it begins");
            }
            let span = json!({"schema_version":1,"kind":"span","id":format!("profile:{key}:range:{}",records.len()),"run_id":run_id,"producer_id":original["producer_id"],"pid":0,"tid":original["tid"],"timestamp_us":start,"duration_us":timestamp-start,"basis":"observed","name":format!("go.{name}"),"evidence":[begin,record["id"]],"attributes":original["attributes"]});
            records.push(span);
        }
    }
    records.push(record);
    Ok(())
}

fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("{name}=");
    let start = line.find(&prefix)? + prefix.len();
    let tail = &line[start..];
    if let Some(rest) = tail.strip_prefix('"') {
        Some(&rest[..rest.find('"')?])
    } else {
        Some(tail.split_whitespace().next().unwrap_or(""))
    }
}

fn metric(line: &str) -> Option<(f64, &str)> {
    let name = field(line, "Name")?;
    let unit = name
        .rsplit_once(':')
        .map(|(_, unit)| unit)
        .unwrap_or("count");
    let value = field(line, "Value")?;
    let number = value.split_once('(')?.1.split_once(')')?.0.parse().ok()?;
    Some((number, unit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    #[test]
    fn clock_snapshot_maps_runtime_events_and_paired_ranges() {
        let text = "M=-1 P=-1 G=-1 Sync Time=1000000 N=1 Trace=1000000 Mono=999000 Wall=2026-10-23T00:00:00Z\nM=5 P=0 G=1 RangeBegin Time=2000000 Name=\"GC pause\" Scope=Goroutine(1)\nM=5 P=0 G=1 RangeEnd Time=3000000 Name=\"GC pause\" Scope=Goroutine(1)\n";
        let (r, _) = parse(text, "run", "key").unwrap();
        let span = r.iter().find(|v| v["kind"] == "span").unwrap();
        assert_eq!(span["duration_us"], 1000);
        assert_eq!(span["timestamp_us"], 1792713600001000u64);
        crate::ingestion::validate("run", &r, &HashSet::new()).unwrap();
    }
}

use super::BrowserStream;
use crate::{Error, Extraction, Limits, Result, record, schema};
use hunin_core::records::{Clock, FieldType, RecordTime};
use serde_json::{Value, json};

pub(super) fn extract(
    profile: &Value,
    subject: &str,
    limits: Limits,
) -> Result<Vec<BrowserStream>> {
    let mut pending = vec![(profile, 0usize)];
    let mut streams = Vec::new();
    let mut remaining_limits = limits;
    while let Some((process, depth)) = pending.pop() {
        if depth > 16 || streams.len() >= 128 {
            return Err(Error("gecko process graph exceeds budget".into()));
        }
        let metadata = process
            .as_object()
            .ok_or_else(|| Error("gecko process must be an object".into()))?;
        if process["meta"]["version"].as_u64() != Some(36) {
            return Err(Error(
                "unsupported gecko profile version; expected verified version36".into(),
            ));
        }
        if process["meta"]["sampleUnits"]["time"] != "ms" {
            return Err(Error(
                "gecko time unit must be explicitly milliseconds".into(),
            ));
        }
        let threads = process["threads"]
            .as_array()
            .ok_or_else(|| Error("gecko threads missing".into()))?;
        let pid = threads
            .first()
            .and_then(|t| t["pid"].as_u64())
            .map(u32::try_from)
            .transpose()
            .map_err(|_| Error("gecko pid overflow".into()))?;
        let key = format!("process-{}", streams.len());
        let mut extraction = Extraction::new(
            remaining_limits,
            json!({"format":"gecko-36","process_id":pid,"scope":"profile-defined process; pid alone does not bind a live OS instance","units":process["meta"]["sampleUnits"],"raw_meta":process["meta"]}),
        );
        extraction.schemas = vec![
            schema(
                "gecko.metadata",
                "profile-defined process/thread fields with original structure",
                &[
                    (
                        "owner",
                        FieldType::String,
                        None,
                        "source-local process or thread identity",
                    ),
                    (
                        "field",
                        FieldType::String,
                        None,
                        "original producer field name",
                    ),
                    (
                        "value",
                        FieldType::Object,
                        None,
                        "one original structured metadata value",
                    ),
                ],
            ),
            schema(
                "gecko.string",
                "source-local string-table entry",
                &[
                    ("thread", FieldType::String, None, "thread table scope"),
                    (
                        "index",
                        FieldType::Unsigned,
                        None,
                        "original string-table id",
                    ),
                    ("text", FieldType::String, None, "exact producer string"),
                ],
            ),
            schema(
                "gecko.table-schema",
                "original gecko table schema and field indices",
                &[
                    ("thread", FieldType::String, None, "thread table scope"),
                    ("table", FieldType::String, None, "producer table name"),
                    ("schema", FieldType::Object, None, "original field indices"),
                ],
            ),
            schema(
                "gecko.table-row",
                "gecko table row preserving all fields and references",
                &[
                    ("thread", FieldType::String, None, "thread table scope"),
                    ("table", FieldType::String, None, "producer table name"),
                    ("index", FieldType::Unsigned, None, "original row index"),
                    (
                        "values",
                        FieldType::Array,
                        None,
                        "original values interpreted by the retained table schema",
                    ),
                ],
            ),
        ];
        extraction.clocks.push(Clock{id:"gecko-relative-us".into(),ticks_per_second:1_000_000,description:"this exported process's profile-relative time, projected from original milliseconds with at most1us rounding; no implicit cross-process mapping".into()});
        for (name, value) in metadata {
            if matches!(name.as_str(), "threads" | "processes") {
                continue;
            }
            store_metadata(&mut extraction, subject, "process", name, value)?;
        }
        for (thread_index, thread) in threads.iter().enumerate() {
            if thread["pid"].as_u64() != pid.map(u64::from) {
                return Err(Error("mixed pid within gecko process".into()));
            }
            let owner = format!("thread-{thread_index}");
            let strings = thread["stringTable"]
                .as_array()
                .ok_or_else(|| Error("gecko string table missing".into()))?;
            if strings.len() > limits.max_records {
                return Err(Error("gecko string table exceeds budget".into()));
            }
            for (index, value) in strings.iter().enumerate() {
                let text = value
                    .as_str()
                    .ok_or_else(|| Error("gecko string table entry must be a string".into()))?;
                if text.len() > limits.max_string_bytes {
                    return Err(Error("gecko string exceeds budget".into()));
                }
                extraction.push(record(
                    format!("{owner}:string:{index}"),
                    "gecko.string",
                    subject,
                    json!({"thread":owner,"index":index,"text":text}),
                ))?;
            }
            let frame = table(thread, "frameTable")?;
            let stacks = table(thread, "stackTable")?;
            let location_column = column(&thread["frameTable"]["schema"], "location")?;
            let frame_column = column(&thread["stackTable"]["schema"], "frame")?;
            let prefix_column = column(&thread["stackTable"]["schema"], "prefix")?;
            let mut positions = Vec::new();
            for row in frame {
                let location_index = index_at(row, location_column, strings.len(), false)?
                    .ok_or_else(|| Error("missing frame location".into()))?;
                positions.push(position(strings[location_index].as_str().unwrap()));
            }
            let mut frame_indices = Vec::new();
            let mut prefixes = Vec::new();
            for row in stacks {
                frame_indices.push(
                    index_at(row, frame_column, frame.len(), false)?
                        .ok_or_else(|| Error("missing stack frame".into()))?,
                );
                prefixes.push(index_at(row, prefix_column, stacks.len(), true)?);
            }
            for start in 0..stacks.len() {
                let mut at = Some(start);
                let mut walked = 0;
                while let Some(index) = at {
                    walked += 1;
                    if walked > 1024 {
                        return Err(Error("gecko stack cycle or excessive depth".into()));
                    }
                    at = prefixes[index];
                }
            }
            for (name, value) in thread
                .as_object()
                .ok_or_else(|| Error("invalid gecko thread".into()))?
            {
                if name == "stringTable" {
                    continue;
                }
                if value.get("schema").is_some() && value.get("data").is_some() {
                    let rows = table(thread, name)?;
                    let columns = value["schema"]
                        .as_object()
                        .ok_or_else(|| Error("gecko table schema must be an object".into()))?;
                    for number in columns.values() {
                        let index = number
                            .as_u64()
                            .ok_or_else(|| Error("table schema indices must be unsigned".into()))?;
                        if index > 256 {
                            return Err(Error("table schema width exceeds budget".into()));
                        }
                    }
                    extraction.push(record(
                        format!("{owner}:{name}:schema"),
                        "gecko.table-schema",
                        subject,
                        json!({"thread":owner,"table":name,"schema":columns}),
                    ))?;
                    for (index, row) in rows.iter().enumerate() {
                        if row.as_array().is_none_or(|row| row.len() > 257) {
                            return Err(Error("invalid gecko table row".into()));
                        }
                        let mut result = record(
                            format!("{owner}:{name}:{index}"),
                            "gecko.table-row",
                            subject,
                            json!({"thread":owner,"table":name,"index":index,"values":row}),
                        );
                        if name == "frameTable" {
                            (result.function, result.location) = positions[index].clone();
                        }
                        if name == "samples" {
                            let time = column(&value["schema"], "time")?;
                            let stack = column(&value["schema"], "stack")?;
                            if let Some(start) = row.get(time).and_then(Value::as_f64) {
                                let micro = start * 1000.;
                                if !micro.is_finite()
                                    || micro < i64::MIN as f64
                                    || micro >= i64::MAX as f64
                                {
                                    return Err(Error(
                                        "gecko sample time outside supported range".into(),
                                    ));
                                }
                                result.time = Some(RecordTime {
                                    clock: "gecko-relative-us".into(),
                                    start: micro.floor() as i64,
                                    end: None,
                                    uncertainty: 1,
                                });
                            }
                            if let Some(mut at) = index_at(row, stack, stacks.len(), true)? {
                                for _ in 0..1024 {
                                    let position = &positions[frame_indices[at]];
                                    if position.0.is_some() {
                                        result.function = position.0.clone();
                                        result.location = position.1.clone();
                                        break;
                                    }
                                    match prefixes[at] {
                                        Some(next) => at = next,
                                        None => break,
                                    }
                                }
                            }
                        }
                        extraction.push(result)?;
                    }
                    let mut extra = value.as_object().unwrap().clone();
                    extra.remove("schema");
                    extra.remove("data");
                    if !extra.is_empty() {
                        store_metadata(
                            &mut extraction,
                            subject,
                            &owner,
                            &format!("{name}:extra"),
                            &Value::Object(extra),
                        )?;
                    }
                } else {
                    store_metadata(&mut extraction, subject, &owner, name, value)?;
                }
            }
        }
        remaining_limits.max_records = remaining_limits
            .max_records
            .checked_sub(extraction.records.len())
            .ok_or_else(|| Error("gecko streams exceed record budget".into()))?;
        remaining_limits.max_output_bytes = remaining_limits
            .max_output_bytes
            .checked_sub(extraction.output_bytes)
            .ok_or_else(|| Error("gecko streams exceed output budget".into()))?;
        streams.push(BrowserStream {
            key,
            process_id: pid,
            extraction,
            interpretation: "browser-gecko36-v1",
        });
        for child in process["processes"].as_array().into_iter().flatten().rev() {
            pending.push((child, depth + 1));
        }
    }
    Ok(streams)
}

fn store_metadata(
    out: &mut Extraction,
    subject: &str,
    owner: &str,
    field: &str,
    value: &Value,
) -> Result<()> {
    if let Some(values) = value.as_array() {
        for (index, value) in values.iter().enumerate() {
            out.push(record(
                format!("{owner}:metadata:{field}:{index}"),
                "gecko.metadata",
                subject,
                json!({"owner":owner,"field":field,"value":{"index":index,"data":value}}),
            ))?;
        }
    } else {
        out.push(record(
            format!("{owner}:metadata:{field}"),
            "gecko.metadata",
            subject,
            json!({"owner":owner,"field":field,"value":{"data":value}}),
        ))?;
    }
    Ok(())
}
fn table<'a>(owner: &'a Value, name: &str) -> Result<&'a Vec<Value>> {
    owner[name]["data"]
        .as_array()
        .ok_or_else(|| Error(format!("gecko {name} data missing")))
}
fn column(schema: &Value, name: &str) -> Result<usize> {
    let value = schema[name]
        .as_u64()
        .ok_or_else(|| Error(format!("gecko {name} column missing")))?;
    usize::try_from(value).map_err(|_| Error("gecko column overflow".into()))
}
fn index_at(row: &Value, column: usize, length: usize, nullable: bool) -> Result<Option<usize>> {
    let value = row.get(column).unwrap_or(&Value::Null);
    if nullable && value.is_null() {
        return Ok(None);
    }
    let number = value
        .as_u64()
        .ok_or_else(|| Error("gecko reference must be unsigned or declared null".into()))?;
    let index = usize::try_from(number).map_err(|_| Error("gecko reference overflow".into()))?;
    if index >= length {
        return Err(Error("gecko reference outside table".into()));
    }
    Ok(Some(index))
}
fn position(location: &str) -> (Option<String>, Option<String>) {
    let Some((function, tail)) = location.rsplit_once(" (") else {
        return (None, None);
    };
    let Some((coordinates, _)) = tail.rsplit_once(')') else {
        return (None, None);
    };
    let Some((prefix, column)) = coordinates.rsplit_once(':') else {
        return (None, None);
    };
    let Some((url, line)) = prefix.rsplit_once(':') else {
        return (None, None);
    };
    if line.parse::<u32>().is_err() || column.parse::<u32>().is_err() || function.is_empty() {
        return (None, None);
    }
    (
        Some(function.to_owned()),
        Some(format!("{url}:{line}:{column}")),
    )
}

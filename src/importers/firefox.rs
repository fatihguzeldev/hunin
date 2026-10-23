use super::{checked_us, sample};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::HashSet, fs, path::Path};

pub(super) fn decode(
    path: &Path,
    raw: &Value,
    run_id: &str,
    key: &str,
) -> Result<(Vec<Value>, Vec<String>)> {
    let sidecar = path.with_extension("syms.json");
    let symbols: Option<Value> = fs::read(&sidecar)
        .ok()
        .map(|data| serde_json::from_slice(&data))
        .transpose()?;
    let mut records = Vec::new();
    let mut unresolved = 0;
    decode_process(
        raw,
        symbols.as_ref(),
        run_id,
        key,
        &mut records,
        &mut unresolved,
    )?;
    let diagnostics = if unresolved > 0 {
        vec![format!(
            "{unresolved} sampled native frames remain unresolved; retain measured binaries/debug data or the samply symbol sidecar"
        )]
    } else {
        Vec::new()
    };
    Ok((records, diagnostics))
}

fn decode_process(
    raw: &Value,
    symbols: Option<&Value>,
    run_id: &str,
    key: &str,
    records: &mut Vec<Value>,
    unresolved: &mut usize,
) -> Result<()> {
    let epoch_ms = raw["meta"]["startTime"].as_f64();
    let interval_us = raw["meta"]["interval"].as_f64().map(|v| v * 1000.0);
    for thread in raw["threads"]
        .as_array()
        .context("Firefox processed profile has no threads")?
    {
        let pid = integer(&thread["pid"]).unwrap_or(0);
        let tid = thread["tid"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| thread["tid"].to_string());
        let table = &thread["samples"];
        if table["weightType"].as_str().is_some_and(|t| t != "samples") {
            bail!(
                "Firefox importer expects sampled-stack weights; this profile has another weightType"
            );
        }
        let stacks = table["stack"]
            .as_array()
            .context("Firefox sample stacks missing")?;
        let times = table["time"]
            .as_array()
            .context("Firefox sample times missing")?;
        if stacks.len() != times.len() {
            bail!("Firefox stack/time lengths differ");
        }
        for (index, leaf) in stacks.iter().enumerate() {
            if leaf.is_null() {
                continue;
            }
            let mut stack = Vec::new();
            let mut chain = Vec::new();
            let mut seen = HashSet::new();
            let mut cursor = Some(leaf.as_u64().context("invalid Firefox stack index")? as usize);
            while let Some(current) = cursor {
                if !seen.insert(current) || seen.len() > 8192 {
                    bail!("cyclic or oversized Firefox stack");
                }
                let frame = thread["stackTable"]["frame"]
                    .get(current)
                    .and_then(Value::as_u64)
                    .context("Firefox stack references missing frame")?;
                chain.push(frame as usize);
                cursor = thread["stackTable"]["prefix"]
                    .get(current)
                    .context("Firefox stack prefix missing")?
                    .as_u64()
                    .map(|v| v as usize);
            }
            for frame in chain.into_iter().rev() {
                let resolved = frame_info(raw, thread, frame, symbols)?;
                *unresolved += resolved
                    .iter()
                    .filter(|f| {
                        f["function"]
                            .as_str()
                            .is_some_and(|n| n.starts_with("0x") || n == "[unresolved]")
                    })
                    .count();
                stack.extend(resolved);
            }
            let time_ms = times[index]
                .as_f64()
                .context("invalid Firefox sample time")?;
            let timestamp = checked_us(
                (epoch_ms.unwrap_or(0.0) + time_ms) * 1000.0,
                "Firefox sample time",
            )?;
            let mut attributes = json!({"source_format":"firefox","thread_name":thread["name"],"sampling_semantics":"OS stack samples; on/off CPU behavior depends on collector OS"});
            if epoch_ms.is_none() {
                attributes["clock"] = json!("profile_relative");
                attributes["clock_alignment"] = json!("unmapped");
            } else {
                attributes["clock"] = json!("unix_epoch");
                attributes["clock_alignment"] = json!("profile_start_epoch_plus_relative_time");
            }
            if let Some(delta) = table["threadCPUDelta"].get(index) {
                attributes["thread_cpu_delta_us"] = delta.clone();
            }
            let weight = table["weight"]
                .get(index)
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            if !weight.is_finite() || weight <= 0.0 {
                continue;
            }
            let mut record = sample(
                run_id,
                key,
                records.len(),
                pid,
                &tid,
                timestamp,
                stack,
                weight,
                "samples",
                attributes,
            );
            if let Some(interval) = interval_us {
                record["interval_us"] = json!(interval);
            }
            records.push(record);
        }
    }
    for child in raw["processes"].as_array().into_iter().flatten() {
        decode_process(child, symbols, run_id, key, records, unresolved)?;
    }
    Ok(())
}

fn frame_info(
    raw: &Value,
    thread: &Value,
    frame: usize,
    sidecar: Option<&Value>,
) -> Result<Vec<Value>> {
    let function = thread["frameTable"]["func"]
        .get(frame)
        .and_then(Value::as_u64)
        .context("Firefox frame function missing")? as usize;
    let name = string_at(thread, &thread["funcTable"]["name"][function]).unwrap_or("[unresolved]");
    let mut result = json!({"function":name});
    for (output, table, field) in [
        ("file", "funcTable", "fileName"),
        ("line", "frameTable", "line"),
        ("column", "frameTable", "column"),
    ] {
        let index = if table == "funcTable" {
            function
        } else {
            frame
        };
        let value = &thread[table][field][index];
        if output == "file" {
            if let Some(s) = string_at(thread, value) {
                result[output] = json!(s);
            }
        } else if let Some(n) = value.as_u64() {
            if output != "line" || n > 0 {
                result[output] = json!(n);
            }
        }
    }
    let address = thread["frameTable"]["address"][frame].as_u64();
    if let Some(address) = address {
        result["address"] = json!(format!("0x{address:x}"));
    }
    let resource = thread["funcTable"]["resource"][function].as_u64();
    let lib = resource
        .and_then(|r| thread["resourceTable"]["lib"][r as usize].as_u64())
        .and_then(|l| raw["libs"].get(l as usize));
    if let Some(lib) = lib {
        if let Some(name) = lib["name"].as_str() {
            result["module"] = json!(name);
        }
        if let Some(code) = lib["codeId"].as_str() {
            result["code_id"] = json!(code);
        }
        if let (Some(sidecar), Some(address)) = (sidecar, address) {
            if let Some(data) = sidecar["data"]
                .as_array()
                .and_then(|a| a.iter().find(|s| same_library(lib, s)))
            {
                if let Some(pair) = data["known_addresses"]
                    .as_array()
                    .and_then(|a| a.iter().find(|p| p[0].as_u64() == Some(address)))
                {
                    let symbol = &data["symbol_table"]
                        [pair[1].as_u64().context("invalid samply symbol index")? as usize];
                    let strings = &sidecar["string_table"];
                    if let Some(name) = symbol["symbol"]
                        .as_u64()
                        .and_then(|i| strings[i as usize].as_str())
                    {
                        result["function"] = json!(name);
                    }
                    if let Some(frames) = symbol["frames"].as_array() {
                        let mut expanded = Vec::new();
                        for inline in frames.iter().rev() {
                            let mut item = result.clone();
                            for (output, input) in [("function", "function"), ("file", "file")] {
                                if let Some(s) = inline[input]
                                    .as_u64()
                                    .and_then(|i| strings[i as usize].as_str())
                                {
                                    item[output] = json!(s);
                                }
                            }
                            if let Some(line) = inline["line"].as_u64().filter(|n| *n > 0) {
                                item["line"] = json!(line);
                            }
                            expanded.push(item);
                        }
                        if !expanded.is_empty() {
                            return Ok(expanded);
                        }
                    }
                }
            }
        }
    }
    Ok(vec![result])
}

fn integer(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

fn same_library(lib: &Value, symbols: &Value) -> bool {
    if let (Some(a), Some(b)) = (lib["codeId"].as_str(), symbols["code_id"].as_str()) {
        if !a.is_empty() && a.eq_ignore_ascii_case(b) {
            return true;
        }
    }
    if let (Some(a), Some(b)) = (lib["breakpadId"].as_str(), symbols["debug_id"].as_str()) {
        let a = a.replace('-', "").to_ascii_lowercase();
        let mut b = b.replace('-', "").to_ascii_lowercase();
        if b.len() == 32 {
            b.push('0');
        }
        return a == b && lib["debugName"].as_str() == symbols["debug_name"].as_str();
    }
    false
}
fn string_at<'a>(thread: &'a Value, index: &Value) -> Option<&'a str> {
    thread["stringArray"]
        .get(index.as_u64()? as usize)?
        .as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_cyclic_stack_instead_of_looping() {
        let raw = json!({"meta":{"startTime":1},"threads":[{"pid":"2","tid":"3","samples":{"stack":[0],"time":[1]},"stackTable":{"frame":[0],"prefix":[0]}}]});
        assert!(decode(Path::new("missing.json"), &raw, "run", "key").is_err());
    }
}

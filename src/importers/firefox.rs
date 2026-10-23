use super::{checked_us, native_identity::Images, sample};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::Path,
};

pub(super) fn decode(
    path: &Path,
    raw: &Value,
    run_id: &str,
    key: &str,
    images: &Images,
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
        images,
    )?;
    let mut diagnostics = images.diagnostics();
    if unresolved > 0 {
        diagnostics.push(format!(
            "{unresolved} sampled native frames remain unresolved; retain measured binaries/debug data or the samply symbol sidecar"
        ));
    }
    Ok((records, diagnostics))
}

fn decode_process(
    raw: &Value,
    symbols: Option<&Value>,
    run_id: &str,
    key: &str,
    records: &mut Vec<Value>,
    unresolved: &mut usize,
    images: &Images,
) -> Result<()> {
    let epoch_ms = raw["meta"]["startTime"].as_f64();
    let interval_us = raw["meta"]["interval"].as_f64().map(|v| v * 1000.0);
    let mut process_lifetimes = HashSet::new();
    for thread in raw["threads"]
        .as_array()
        .context("Firefox processed profile has no threads")?
    {
        let pid = integer(&thread["pid"]).unwrap_or(0);
        let tid = thread["tid"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| thread["tid"].to_string());
        let relative_start = thread["processStartupTime"].as_f64();
        let lifetime = relative_start
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".into());
        let producer = format!("profile:{key}:{pid}:{lifetime}");
        if let (Some(start), Some(end)) = (relative_start, thread["processShutdownTime"].as_f64()) {
            if end < start {
                bail!("Firefox process interval ends before it starts");
            }
            if process_lifetimes.insert((pid, lifetime.clone())) {
                let timestamp = checked_us(
                    (epoch_ms.unwrap_or(0.0) + start) * 1000.0,
                    "Firefox process start",
                )?;
                let duration =
                    checked_us((end - start) * 1000.0, "Firefox process observed duration")?;
                let mut attrs = json!({"source_format":"firefox","scope":"process","process_name":thread["processName"],"lifetime_scope":"collector-observed process interval"});
                if epoch_ms.is_some() {
                    attrs["clock"] = json!("unix_epoch");
                    attrs["clock_alignment"] = json!("profile_start_epoch_plus_relative_time");
                } else {
                    attrs["clock"] = json!("profile_relative");
                    attrs["clock_alignment"] = json!("unmapped");
                }
                records.push(json!({"schema_version":1,"kind":"span","id":format!("profile:{key}:process:{pid}:{lifetime}"),"run_id":run_id,"producer_id":producer,"pid":pid,"tid":"process","timestamp_us":timestamp,"duration_us":duration,"basis":"observed","name":"process.observed_lifetime","attributes":attrs}));
            }
        }
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
            let mut image_checks = BTreeMap::new();
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
                let resolved = frame_info(raw, thread, frame, symbols, images, &mut image_checks)?;
                *unresolved += resolved
                    .iter()
                    .filter(|f| {
                        f["function"]
                            .as_str()
                            .is_some_and(|n| n.starts_with("0x") || n.starts_with("[unresolved"))
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
            attributes["native_image_checks"] =
                json!(image_checks.into_values().collect::<Vec<_>>());
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
            record["producer_id"] = json!(producer);
            if let Some(interval) = interval_us {
                record["interval_us"] = json!(interval);
            }
            records.push(record);
        }
    }
    for child in raw["processes"].as_array().into_iter().flatten() {
        decode_process(child, symbols, run_id, key, records, unresolved, images)?;
    }
    Ok(())
}

fn frame_info(
    raw: &Value,
    thread: &Value,
    frame: usize,
    sidecar: Option<&Value>,
    images: &Images,
    checks: &mut BTreeMap<String, Value>,
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
        if let Some(check) = images.get(lib) {
            checks.insert(lib.to_string(), check.clone());
            if check["status"] == "mismatch" {
                let frame = result.as_object_mut().unwrap();
                for field in ["file", "line", "column"] {
                    frame.remove(field);
                }
                result["function"] = json!("[unresolved: native image identity mismatch]");
                return Ok(vec![result]);
            }
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
        if !a.is_empty() && !b.is_empty() {
            return a.eq_ignore_ascii_case(b);
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
        assert!(
            decode(
                Path::new("missing.json"),
                &raw,
                "run",
                "key",
                &Images::inspect(&raw)
            )
            .is_err()
        );
    }
    #[test]
    fn code_identifier_conflicts_cannot_fall_back_to_matching_debug_identifiers() {
        assert!(!same_library(
            &json!({"codeId":"A","breakpadId":"abcd0","debugName":"same"}),
            &json!({"code_id":"B","debug_id":"abcd","debug_name":"same"})
        ));
    }
    #[test]
    fn actual_unloaded_reloaded_image_mismatch_cannot_borrow_another_images_source() {
        let directory = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            (
                "phase_a.dylib",
                include_bytes!("fixtures/native-reload/phase_a.macho").as_slice(),
            ),
            (
                "phase_b.dylib",
                include_bytes!("fixtures/native-reload/phase_b.macho").as_slice(),
            ),
        ] {
            fs::write(directory.path().join(name), bytes).unwrap();
        }
        let mut raw: Value =
            serde_json::from_slice(include_bytes!("fixtures/native-reload/cpu.json")).unwrap();
        for lib in raw["libs"].as_array_mut().unwrap() {
            if lib["name"]
                .as_str()
                .is_some_and(|s| s.starts_with("phase_"))
            {
                lib["path"] = json!(directory.path().join(lib["name"].as_str().unwrap()));
            }
        }
        let path = directory.path().join("cpu.json");
        fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        fs::write(
            path.with_extension("syms.json"),
            include_bytes!("fixtures/native-reload/cpu.syms.json"),
        )
        .unwrap();
        let unchecked = decode(
            &path,
            &raw,
            "reload-control",
            "unchecked",
            &Images::default(),
        )
        .unwrap();
        assert!(
            unchecked
                .0
                .iter()
                .flat_map(|r| r["stack"].as_array().into_iter().flatten())
                .any(|f| f["module"] == "phase_b.dylib"
                    && f["file"].as_str().is_some_and(|s| s.ends_with("phase_a.c"))),
            "control must expose the raw wrong-image symbol match"
        );
        let profile = crate::importers::import(&path, "firefox", "reload-control").unwrap();
        assert!(
            profile
                .diagnostics
                .iter()
                .any(|d| d.contains("identity mismatch for phase_b.dylib"))
        );
        let frames: Vec<_> = profile
            .records
            .iter()
            .flat_map(|r| r["stack"].as_array().into_iter().flatten())
            .collect();
        let a: Vec<_> = frames
            .iter()
            .filter(|f| f["module"] == "phase_a.dylib")
            .collect();
        let b: Vec<_> = frames
            .iter()
            .filter(|f| f["module"] == "phase_b.dylib")
            .collect();
        assert!(a.len() > 100 && b.len() > 100);
        assert!(a.iter().any(|f| f["function"] == "phase_hot"
            && f["file"].as_str().is_some_and(|s| s.ends_with("phase_a.c"))));
        assert!(b.iter().all(
            |f| f["function"] == "[unresolved: native image identity mismatch]"
                && f.get("file").is_none()
                && f.get("line").is_none()
                && f["code_id"] == "CD3C90FFC4B93EF1B5FD3FBD731EA331"
        ));
        crate::ingestion::validate("reload-control", &profile.records, &HashSet::new()).unwrap();
        assert_eq!(
            profile.records,
            crate::importers::import(&path, "firefox", "reload-control")
                .unwrap()
                .records
        );
        fs::write(
            directory.path().join("phase_b.dylib"),
            include_bytes!("fixtures/native-reload/phase_a.macho"),
        )
        .unwrap();
        let changed = crate::importers::import(&path, "firefox", "reload-control").unwrap();
        assert_ne!(
            profile.input_fingerprint, changed.input_fingerprint,
            "image identity affects normalization/replay identity"
        );
    }
}

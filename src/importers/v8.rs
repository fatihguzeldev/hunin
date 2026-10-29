use super::StackSample;
use super::{checked_us, javascript_sources::Scripts, sample};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

pub(super) fn decode(
    path: &Path,
    raw: &Value,
    run_id: &str,
    key: &str,
    sources: &Scripts,
) -> Result<(Vec<Value>, Vec<String>)> {
    let nodes = raw["nodes"]
        .as_array()
        .context("CPU profile nodes missing")?;
    let mut map = HashMap::new();
    let mut parents = HashMap::new();
    for node in nodes {
        let id = node["id"].as_u64().context("CPU profile node id missing")?;
        if map.insert(id, node).is_some() {
            bail!("duplicate CPU profile node id");
        }
        for child in node["children"].as_array().into_iter().flatten() {
            let child = child.as_u64().context("invalid child node")?;
            if parents.insert(child, id).is_some() {
                bail!("CPU profile node has multiple parents");
            }
        }
    }
    let (filename_pid, filename_tid) = identity_from_filename(path);
    let metadata: Value = metadata_paths(path)
        .iter()
        .find_map(|p| fs::read(p).ok())
        .map(|v| serde_json::from_slice(&v))
        .transpose()?
        .unwrap_or(json!({}));
    let origin = metadata["profile_time_origin_us"].as_f64();
    let pid = metadata["pid"].as_u64().or(filename_pid).unwrap_or(0);
    let tid = filename_tid
        .or_else(|| metadata["tid"].as_str().map(str::to_owned))
        .or_else(|| metadata["tid"].as_u64().map(|n| n.to_string()))
        .unwrap_or_else(|| "unknown".into());
    let samples = raw["samples"]
        .as_array()
        .context("CPU profile samples missing")?;
    let deltas = raw["timeDeltas"]
        .as_array()
        .context("CPU profile timeDeltas missing")?;
    if samples.len() != deltas.len() {
        bail!("CPU profile sample/timeDelta lengths differ");
    }
    let start = raw["startTime"]
        .as_f64()
        .context("CPU profile startTime missing")?;
    checked_us(start, "CPU profile startTime")?;
    let end = raw
        .get("endTime")
        .filter(|value| !value.is_null())
        .map(|value| -> Result<f64> {
            let end = value.as_f64().context("invalid CPU profile endTime")?;
            checked_us(end, "CPU profile endTime")?;
            Ok(end)
        })
        .transpose()?;
    let mut now = start;
    let mut backsteps = 0;
    let mut outside_bounds = 0;
    let mut records = Vec::new();
    let mut frames = HashMap::new();
    for (id, node) in &map {
        let call = &node["callFrame"];
        let name = call["functionName"].as_str().unwrap_or("[unknown]");
        let mut frame = json!({"function":if name.is_empty(){"(anonymous)"}else{name}});
        if let Some(url) = call["url"].as_str().filter(|s| !s.is_empty()) {
            frame["file"] = json!(url);
        }
        if let Some(line) = call["lineNumber"].as_i64().filter(|n| *n >= 0) {
            frame["line"] = json!(line + 1);
        }
        if let Some(column) = call["columnNumber"].as_i64().filter(|n| *n >= 0) {
            frame["column"] = json!(column);
        }
        let proof = sources.frame(call, &mut frame);
        frames.insert(*id, (frame, proof));
    }
    for (index, leaf) in samples.iter().enumerate() {
        let delta = deltas[index]
            .as_f64()
            .context("invalid CPU profile time delta")?;
        // V8 serializes signed differences between consecutive sample times.
        // DevTools also accumulates these before ordering samples for display:
        // a negative difference is ordering evidence, not negative CPU cost.
        // Keep the original index, timestamp and sample-count weight here.
        backsteps += usize::from(delta < 0.0);
        now += delta;
        checked_us(now, "CPU profile sample timestamp")?;
        outside_bounds += usize::from(now < start || end.is_some_and(|end| now > end));
        let mut cursor = Some(leaf.as_u64().context("invalid CPU profile leaf")?);
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut script_proofs = std::collections::BTreeMap::new();
        while let Some(id) = cursor {
            if !seen.insert(id) || seen.len() > 8192 {
                bail!("cyclic or oversized CPU profile ancestry");
            }
            let (frame, proof) = frames
                .get(&id)
                .context("sample references missing CPU profile node")?;
            if !proof.is_null() {
                script_proofs.insert(id, proof.clone());
            }
            chain.push(frame.clone());
            cursor = parents.get(&id).copied();
        }
        chain.reverse();
        let timestamp = checked_us(now + origin.unwrap_or(0.0), "CPU sample timestamp")?;
        let mut attrs = if origin.is_some() {
            json!({"source_format":"v8-cpuprofile","clock":"unix_epoch","clock_alignment":"runtime_anchor","clock_source":metadata["clock_source"]})
        } else {
            json!({"source_format":"v8-cpuprofile","clock":"runtime_monotonic","clock_alignment":"unmapped"})
        };
        attrs["profile_time_delta_us"] = deltas[index].clone();
        attrs["profile_timestamp_us"] = json!(now);
        if !script_proofs.is_empty() {
            attrs["script_sources"] = json!(script_proofs.into_values().collect::<Vec<_>>());
        }
        records.push(sample(
            run_id,
            key,
            index,
            StackSample {
                pid,
                tid: &tid,
                timestamp_us: timestamp,
                stack: chain,
                weight: 1.0,
                weight_unit: "samples",
                attributes: attrs,
            },
        ));
    }
    let mut diagnostics = if origin.is_none() {
        vec!["CPU profile clock has no verified epoch anchor; sample times remain outside the shared epoch timeline".into()]
    } else {
        Vec::new()
    };
    if backsteps > 0 {
        diagnostics.push(format!("CPU profile contains {backsteps} timestamp backstep(s); original signed deltas, sample order and timestamps are preserved; weights remain sample counts"));
    }
    if outside_bounds > 0 {
        diagnostics.push(format!("CPU profile contains {outside_bounds} sample timestamps outside its declared start/end range; reported timestamps are preserved, not clamped"));
    }
    diagnostics.extend(sources.diagnostics.clone());
    Ok((records, diagnostics))
}
pub(super) fn script_paths(path: &Path) -> Vec<std::path::PathBuf> {
    let (Some(pid), Some(tid)) = identity_from_filename(path) else {
        return Vec::new();
    };
    vec![
        path.parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("scripts-{pid}-{tid}.jsonl")),
    ]
}

pub(super) fn metadata_paths(path: &Path) -> Vec<std::path::PathBuf> {
    let (pid, tid) = identity_from_filename(path);
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let mut candidates = vec![
        path.with_extension("meta.json"),
        std::path::PathBuf::from(format!("{}.meta.json", path.display())),
    ];
    if let Some(pid) = pid {
        if let Some(tid) = tid {
            candidates.push(directory.join(format!("clock-{pid}-{tid}.meta.json")));
        }
        if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().contains(".worker-"))
        {
            candidates.push(directory.join(format!("clock-{pid}-0.meta.json")));
        }
    }
    candidates
}

fn identity_from_filename(path: &Path) -> (Option<u64>, Option<String>) {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return (None, None);
    };
    let parts: Vec<_> = name.split('.').collect();
    if parts.first() == Some(&"CPU")
        && parts.len() >= 7
        && parts[1].len() == 8
        && parts[2].len() == 6
    {
        (
            parts.get(3).and_then(|s| s.parse().ok()),
            parts.get(4).map(|s| s.to_string()),
        )
    } else if parts.first() == Some(&"CPU") && parts.len() >= 4 && parts[1].len() >= 12 {
        (
            parts.get(2).and_then(|s| s.parse().ok()),
            Some(
                parts
                    .get(3)
                    .filter(|s| s.starts_with("worker-"))
                    .copied()
                    .unwrap_or("0")
                    .to_owned(),
            ),
        )
    } else if parts.first() == Some(&"CPU") && parts.len() == 6 {
        (parts[1].parse().ok(), Some(parts[2].to_owned()))
    } else {
        (None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_deltas_keep_sample_identity_order_and_count_weight() {
        // Reduced from Deno 2.9.8's Mac ARM profile: an anonymous sample is
        // timestamped 2us before the preceding hotspot sample.
        let raw = json!({"startTime":366793600,"endTime":367275292,
            "nodes":[{"id":45,"callFrame":{"functionName":""},"children":[46]},
                     {"id":46,"callFrame":{"functionName":"huninSmokeHotspot"}}],
            "samples":[46,45,46],"timeDeltas":[83524,-2,168]});
        let (records, diagnostics) = decode(
            Path::new("absent.cpuprofile"),
            &raw,
            "run",
            "key",
            &Scripts::default(),
        )
        .unwrap();
        let timestamps: Vec<_> = records
            .iter()
            .map(|row| row["timestamp_us"].as_u64().unwrap())
            .collect();
        assert_eq!(timestamps, [366877124, 366877122, 366877290]);
        assert_eq!(records[0]["stack"][1]["function"], "huninSmokeHotspot");
        assert_eq!(records[1]["stack"][0]["function"], "(anonymous)");
        assert_eq!(records[1]["id"], "profile:key:1");
        assert_eq!(records[1]["attributes"]["profile_time_delta_us"], -2);
        assert_eq!(
            records[1]["attributes"]["profile_timestamp_us"].as_f64(),
            Some(366877122.0)
        );
        assert!(
            records
                .iter()
                .all(|row| row["weight"] == 1.0 && row["weight_unit"] == "samples")
        );
        assert!(
            diagnostics
                .iter()
                .any(|message| message.contains("1 timestamp backstep"))
        );
    }

    #[test]
    fn declared_time_bounds_are_evidence_rather_than_a_timestamp_rewrite() {
        let raw = json!({"startTime":10,"endTime":20,
            "nodes":[{"id":1,"callFrame":{"functionName":"work"}}],
            "samples":[1,1],"timeDeltas":[12,-13]});
        let (records, diagnostics) = decode(
            Path::new("absent.cpuprofile"),
            &raw,
            "run",
            "key",
            &Scripts::default(),
        )
        .unwrap();
        assert_eq!(records[0]["timestamp_us"], 22);
        assert_eq!(records[1]["timestamp_us"], 9);
        assert!(
            diagnostics
                .iter()
                .any(|message| message.contains("2 sample timestamps outside"))
        );
    }

    #[test]
    fn fractional_runtime_times_are_preserved_before_protocol_rounding() {
        let raw = json!({"startTime":10.125,"endTime":20,
            "nodes":[{"id":1,"callFrame":{"functionName":"work"}}],
            "samples":[1,1],"timeDeltas":[1.25,-0.125]});
        let (records, _) = decode(
            Path::new("absent.cpuprofile"),
            &raw,
            "run",
            "key",
            &Scripts::default(),
        )
        .unwrap();
        assert_eq!(records[0]["attributes"]["profile_timestamp_us"], 11.375);
        assert_eq!(records[1]["attributes"]["profile_timestamp_us"], 11.25);
        assert_eq!(records[1]["attributes"]["profile_time_delta_us"], -0.125);
        assert_eq!(records[0]["timestamp_us"], 11);
        assert_eq!(records[1]["weight"], 1.0);
    }

    #[test]
    fn signed_deltas_still_reject_unsafe_or_negative_cumulative_timestamps() {
        for (start, end, deltas) in [
            (json!(10), json!(20), json!([-11])),
            (json!(9_007_199_254_740_991u64), Value::Null, json!([1])),
            (json!(9_007_199_254_740_992u64), Value::Null, json!([-1])),
            (json!(10), json!(9_007_199_254_740_992u64), json!([1])),
            (json!(10), json!("invalid"), json!([1])),
        ] {
            let raw = json!({"startTime":start,"endTime":end,
                "nodes":[{"id":1,"callFrame":{"functionName":"work"}}],
                "samples":[1],"timeDeltas":deltas});
            assert!(
                decode(
                    Path::new("absent.cpuprofile"),
                    &raw,
                    "run",
                    "key",
                    &Scripts::default()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn raw_profile_retains_unmapped_clock_and_root_to_leaf_stack() {
        let raw = json!({"startTime":10,"nodes":[{"id":1,"callFrame":{"functionName":"root"},"children":[2]},{"id":2,"callFrame":{"functionName":"hot","url":"work.ts","lineNumber":4}}],"samples":[2],"timeDeltas":[3]});
        let (r, d) = decode(
            Path::new("absent.cpuprofile"),
            &raw,
            "run",
            "key",
            &Scripts::default(),
        )
        .unwrap();
        assert_eq!(r[0]["timestamp_us"], 13);
        assert_eq!(r[0]["stack"][0]["function"], "root");
        assert_eq!(r[0]["stack"][1]["line"], 5);
        assert_eq!(r[0]["attributes"]["clock_alignment"], "unmapped");
        assert!(!d.is_empty());
    }
}

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
    let mut now = raw["startTime"]
        .as_f64()
        .context("CPU profile startTime missing")?;
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
        if delta < 0.0 {
            bail!("negative CPU profile sample delta");
        }
        now += delta;
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
        if !script_proofs.is_empty() {
            attrs["script_sources"] = json!(script_proofs.into_values().collect::<Vec<_>>());
        }
        records.push(sample(
            run_id, key, index, pid, &tid, timestamp, chain, 1.0, "samples", attrs,
        ));
    }
    let mut diagnostics = if origin.is_none() {
        vec!["CPU profile clock has no verified epoch anchor; sample times remain outside the shared epoch timeline".into()]
    } else {
        Vec::new()
    };
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

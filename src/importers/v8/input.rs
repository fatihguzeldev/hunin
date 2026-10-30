//! Sidecar discovery and runtime clock/producer metadata for a V8 recording.
use anyhow::Result;
use serde_json::{Value, json};
use std::{fs, path::Path};

pub(super) fn metadata(path: &Path) -> Result<Value> {
    Ok(metadata_paths(path)
        .iter()
        .find_map(|p| fs::read(p).ok())
        .map(|v| serde_json::from_slice(&v))
        .transpose()?
        .unwrap_or(json!({})))
}

pub(in crate::importers) fn script_paths(path: &Path) -> Vec<std::path::PathBuf> {
    let (Some(pid), Some(tid)) = identity_from_filename(path) else {
        return Vec::new();
    };
    vec![
        path.parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("scripts-{pid}-{tid}.jsonl")),
    ]
}

pub(in crate::importers) fn metadata_paths(path: &Path) -> Vec<std::path::PathBuf> {
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

pub(super) fn identity_from_filename(path: &Path) -> (Option<u64>, Option<String>) {
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

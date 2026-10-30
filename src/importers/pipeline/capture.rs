//! Snapshot raw bytes and ordered format sidecars before interpretation.
use crate::importers::{firefox, formats::ProfileFormat, inputs, v8};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub(super) fn capture_inputs(
    path: &Path,
    format: &str,
    limits: inputs::Limits,
    deadline: Option<std::time::Instant>,
) -> Result<(inputs::Snapshot, Sha256)> {
    let kind = ProfileFormat::parse(format)?;
    let mut snapshot = inputs::Snapshot::new(limits, deadline)?;
    let mut digest = Sha256::new();
    digest.update(b"hunin-import-v9\0");
    digest.update(format.as_bytes());
    digest.update([0]);
    snapshot.capture_raw(path, &mut digest)?;
    if kind == ProfileFormat::V8 {
        digest.update(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .as_bytes(),
        );
    }
    let scripts = if kind == ProfileFormat::V8 {
        v8::script_paths(path)
    } else {
        vec![]
    };
    for sidecar in sidecars(path, kind) {
        digest.update([0]);
        digest.update(
            sidecar
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .as_bytes(),
        );
        let maximum = if scripts.contains(&sidecar) {
            v8::MAX_SOURCE_BYTES
        } else if kind == ProfileFormat::Firefox && sidecar == path.with_extension("lifetimes.json")
        {
            firefox::MAX_LIFETIME_SIDECAR
        } else {
            limits.sidecar_bytes
        };
        snapshot.capture_sidecar(&sidecar, maximum, &mut digest)?;
    }
    Ok((snapshot, digest))
}

fn sidecars(path: &Path, format: ProfileFormat) -> Vec<PathBuf> {
    let candidates = match format {
        ProfileFormat::Firefox => vec![
            path.with_extension("syms.json"),
            path.with_extension("lifetimes.json"),
        ],
        ProfileFormat::V8 => v8::metadata_paths(path)
            .into_iter()
            .chain(v8::script_paths(path))
            .collect(),
        ProfileFormat::Graal => vec![path.with_extension("meta.json")],
        ProfileFormat::Vmprof => vec![PathBuf::from(format!("{}.meta.json", path.display()))],
        ProfileFormat::PythonCpuProgress => {
            vec![PathBuf::from(format!("{}.meta.json", path.display()))]
        }
        ProfileFormat::GoTrace => vec![
            path.parent()
                .unwrap_or_else(|| Path::new("."))
                .join("go-tool.json"),
        ],
        ProfileFormat::Pprof | ProfileFormat::Jsonl => Vec::new(),
    };
    let mut result = candidates
        .into_iter()
        .filter(|p| p.is_file())
        .collect::<Vec<_>>();
    result.sort();
    result.dedup();
    result
}

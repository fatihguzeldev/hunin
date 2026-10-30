//! Inventory and classify retained raw bytes without decoding or completeness claims.
use super::CaptureArtifact;
use crate::host::walk_files;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Preserve these artifacts before checking completeness or decoding profile bytes.
pub fn capture_artifacts(directory: &Path) -> Result<Vec<CaptureArtifact>> {
    let mut files = Vec::new();
    walk_files(directory, &mut files)?;
    files.sort();
    Ok(artifacts_from_paths(files))
}

pub(crate) fn artifacts_from_paths(files: Vec<PathBuf>) -> Vec<CaptureArtifact> {
    files
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy();
            let format = profile_format(&path)
                .unwrap_or_else(|| {
                    if name.ends_with(".jsonl") {
                        "hunin-jsonl"
                    } else if name.ends_with(".meta.json")
                        || name.ends_with(".syms.json")
                        || name == "go-tool.json"
                    {
                        "profile-sidecar"
                    } else if name == "stdout.txt" {
                        "stdout"
                    } else if name == "stderr.txt" {
                        "stderr"
                    } else {
                        "collector-artifact"
                    }
                })
                .to_owned();
            CaptureArtifact { path, format }
        })
        .collect()
}

pub(super) fn profile_format(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_string_lossy();
    if name.ends_with(".calls.jsonl") {
        Some("hunin-jsonl")
    } else if name.ends_with(".pythoncpu.jsonl") {
        Some("python-cpu-progress")
    } else if name.ends_with(".cpuprofile") {
        Some("v8-cpuprofile")
    } else if name.ends_with(".vmprof") {
        Some("vmprof")
    } else if name.ends_with(".samply.json") || name.ends_with(".samply.json.gz") {
        Some("firefox")
    } else if name.ends_with(".graal.json") {
        Some("graal-cpusampler")
    } else if name.ends_with(".pprof") || name.ends_with(".pprof.gz") {
        Some("pprof")
    } else if name.ends_with(".gotrace") {
        Some("go-trace")
    } else {
        None
    }
}

use crate::model::{code::ArtifactRef, runs::Artifact};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub(super) fn path(parent: &Path, name: &str) -> Result<PathBuf> {
    ensure!(
        !name.contains(['\\', ':'])
            && matches!(
                Path::new(name).components().collect::<Vec<_>>().as_slice(),
                [Component::Normal(_)]
            ),
        "V8 code sidecar must have one safe filename"
    );
    Ok(parent.join(name))
}
pub(super) fn sidecar_paths(manifest: &Path, raw: &Value) -> Result<Vec<PathBuf>> {
    ensure!(raw["schema_version"] == 1, "unsupported V8 code manifest");
    let parent = manifest.parent().context("V8 manifest parent missing")?;
    let mut names = vec![];
    for key in ["logs", "wasm", "compiler_exports"] {
        let rows = raw[key]
            .as_array()
            .context("V8 manifest inventory array missing")?;
        ensure!(rows.len() <= 1024, "V8 inventory entry limit exceeded");
        for row in rows {
            names.push(
                row["path"]
                    .as_str()
                    .context("V8 inventory filename missing")?,
            );
            if key == "logs"
                && let Some(ll) = row["ll_path"].as_str()
            {
                names.push(ll);
            }
        }
    }
    if let Some(published) = raw["published_code"]["path"].as_str() {
        names.push(published);
    }
    let mut paths = names
        .into_iter()
        .map(|name| path(parent, name))
        .collect::<Result<Vec<_>>>()?;
    paths.sort();
    paths.dedup();
    ensure!(
        !paths.iter().any(|path| path == manifest),
        "V8 manifest cannot inventory itself"
    );
    Ok(paths)
}
pub(super) fn read(parent: &Path, name: &str, maximum: u64) -> Result<Vec<u8>> {
    let path = path(parent, name)?;
    let metadata = fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "V8 sidecar is not a regular retained file"
    );
    ensure!(
        metadata.len() <= maximum,
        "V8 artifact byte budget exceeded"
    );
    let bytes = fs::read(path)?;
    ensure!(
        bytes.len() as u64 <= maximum,
        "V8 sidecar grew past byte budget"
    );
    Ok(bytes)
}
pub(super) fn reference(name: &str, bytes: &[u8], format: &str) -> ArtifactRef {
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    ArtifactRef {
        artifact_id: Artifact::reference_id(&sha256, name, format),
        sha256,
        offset: None,
        length: None,
    }
}

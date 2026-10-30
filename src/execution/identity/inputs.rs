//! Explicit workload input files snapshotted before launch.
use super::file_hash;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(in crate::execution) fn workload_inputs(
    cwd: &Path,
    paths: &[PathBuf],
) -> anyhow::Result<Value> {
    use anyhow::Context;
    let mut files = BTreeMap::new();
    for path in paths {
        let actual = cwd
            .join(path)
            .canonicalize()
            .with_context(|| format!("declared input does not exist: {}", path.display()))?;
        let metadata = fs::metadata(&actual)?;
        if !metadata.is_file() {
            anyhow::bail!("declared input must be a regular file: {}", path.display());
        }
        let key = actual
            .strip_prefix(cwd)
            .unwrap_or(&actual)
            .to_string_lossy()
            .to_string();
        files.insert(
            key.clone(),
            json!({"path":key,"sha256":file_hash(&actual)?,"bytes":metadata.len()}),
        );
    }
    let rows = files.into_values().collect::<Vec<_>>();
    Ok(
        json!({"fingerprint":format!("{:x}",Sha256::digest(serde_json::to_vec(&rows)?)),"files":rows,"coverage":"explicit files snapshotted before launch only; stdin is closed, dynamic or network inputs are not inferred"}),
    )
}

//! Observed V8 native ticks, code lifetimes and retained compiler exports.
mod compiler;
mod compiler_native;
mod compiler_source;
mod evidence;
mod ll;
mod normalize;
mod raw;
mod wasm;

#[cfg(test)]
mod tests;

use super::output::RecordBatch;
use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn sidecar_paths(path: &Path, raw: &Value) -> Result<Vec<PathBuf>> {
    evidence::sidecar_paths(path, raw)
}
pub(super) fn decode_into(
    path: &Path,
    raw: &Value,
    run_id: &str,
    key: &str,
    records: &mut RecordBatch<'_>,
) -> Result<Vec<String>> {
    normalize::decode_into(path, raw, run_id, key, records)
}

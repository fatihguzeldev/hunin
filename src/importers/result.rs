//! Normalization result and retained source/image evidence contracts.
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone)]
pub struct SourceSnapshot {
    pub uri: String,
    pub content: String,
    pub sha256: String,
    pub provenance: Value,
}
#[derive(Debug, Clone)]
pub struct BinarySnapshot {
    pub sha256: String,
    pub content: Vec<u8>,
}
#[derive(Debug, Clone)]
pub struct ImportedProfile {
    pub source: PathBuf,
    pub raw_sha256: String,
    pub format: String,
    pub records: Vec<Value>,
    pub diagnostics: Vec<String>,
    pub input_fingerprint: String,
    pub sidecars: Vec<PathBuf>,
    pub sidecar_sha256: BTreeMap<PathBuf, String>,
    pub source_snapshots: Vec<SourceSnapshot>,
    pub binary_snapshots: Vec<BinarySnapshot>,
    pub native_image_context: Option<Value>,
    /// Actual offline decoder identity and resolved execution paths, not capture identity.
    pub decoder_provenance: Option<Value>,
}

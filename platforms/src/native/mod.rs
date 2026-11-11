use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod control;
pub mod coverage;
pub mod etw;
pub mod macos;
pub mod perf;
mod recording;
mod symbol_budget;
pub mod symbols;
#[cfg(windows)]
pub mod windows;
pub mod xctrace;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    C,
    Cpp,
    Rust,
    Go,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub path: PathBuf,
    pub architecture: String,
    pub identity_kind: String,
    pub identity: String,
    pub load_address: u64,
    pub file_offset: u64,
    pub mapped_virtual_address: Option<u64>,
    pub end_address: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub column: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineSite {
    pub function: Option<String>,
    pub source: Option<SourceLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub address: u64,
    pub alternate_addresses: Vec<u64>,
    pub addresses_truncated: bool,
    pub image: Option<usize>,
    pub function: Option<String>,
    pub reported_function: Option<String>,
    pub inline_sites: Vec<InlineSite>,
    pub source: Option<SourceLocation>,
    pub resolution: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub instruction_pointer: u64,
    pub producer_fields: serde_json::Value,
    pub process_id: u32,
    pub timestamp: u64,
    pub thread_id: u64,
    pub cpu: Option<u32>,
    pub weight: u64,
    pub weight_unit: String,
    pub frames: Vec<Frame>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateStack {
    pub id: u64,
    pub parent: Option<u64>,
    pub thread_id: Option<u64>,
    pub count: u64,
    pub frame: Frame,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub schema: String,
    pub producer: String,
    pub language: Language,
    pub images: Vec<Image>,
    pub samples: Vec<Sample>,
    pub aggregates: Vec<AggregateStack>,
    pub clock: Option<String>,
    pub ticks_per_second: u64,
    pub period: Option<u64>,
    pub start: Option<u64>,
    pub end: Option<u64>,
    pub issues: Vec<String>,
    pub metadata: serde_json::Value,
}

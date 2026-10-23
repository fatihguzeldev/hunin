use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub name: String,
    pub status: String,
    pub command: Vec<String>,
    pub cwd: String,
    pub runtime: String,
    pub started_at_us: u64,
    pub finished_at_us: Option<u64>,
    pub exit_code: Option<i32>,
    pub diagnostics: Vec<String>,
    pub source: Value,
    pub correctness: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub format: String,
}

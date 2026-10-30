//! Recording requests and lifecycle composition.
mod admission;
mod correctness;
mod engine;
mod finishing;
mod identity;
mod launch;
mod lifecycle;
mod recovery;
mod resources;
mod retention;
mod sdk;
mod supervision;

pub use engine::Engine;

use anyhow::{Context, Result};
use serde::Deserialize;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRequest {
    pub name: Option<String>,
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub runtime: Option<String>,
    pub timeout_seconds: Option<f64>,
    pub expected_exit_code: Option<i32>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub input_files: Vec<PathBuf>,
    pub expected_stdout: Option<String>,
    pub expected_stdout_sha256: Option<String>,
    /// Explicit opt-in; absence preserves ordinary automatic CPU sampling.
    pub call_counting: Option<crate::model::calls::CountRequest>,
}

struct CountOrigin {
    parent_run_id: String,
    request_key: String,
}

/// Owned blocking units settle before their recording releases admission.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .context("recording blocking unit failed")?
}

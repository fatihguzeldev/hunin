//! Collector inputs, launch plans and raw-delivery/completeness contracts.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureMode {
    #[default]
    AutomaticCpu,
    CodeOrigin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CodeOptions {
    pub compiler_export: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_function_index: Option<u32>,
    pub sampling_interval_us: u32,
    pub max_artifact_bytes: u64,
}

impl Default for CodeOptions {
    fn default() -> Self {
        Self {
            compiler_export: false,
            function_filter: None,
            wasm_function_index: None,
            sampling_interval_us: 1000,
            max_artifact_bytes: 64 * 1024 * 1024,
        }
    }
}

impl CodeOptions {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.wasm_function_index.is_none() || self.compiler_export,
            "wasm_function_index requires compiler_export"
        );
        anyhow::ensure!(
            self.wasm_function_index
                .is_none_or(|index| index <= 2_147_483_647),
            "Wasm disassembly function index must be 0..2147483647"
        );
        anyhow::ensure!(
            (100..=1_000_000).contains(&self.sampling_interval_us),
            "code sampling interval must be 100..1000000 microseconds"
        );
        anyhow::ensure!(
            (1..=536_870_912).contains(&self.max_artifact_bytes),
            "code artifact budget must be 1..536870912 bytes"
        );
        if let Some(filter) = &self.function_filter {
            anyhow::ensure!(
                !filter.is_empty() && filter.len() <= 256 && !filter.contains(['\0', '\n', '\r']),
                "code function filter must be 1..256 bytes without NUL or newlines"
            );
        }
        Ok(())
    }
}

impl CaptureMode {
    pub fn validate_options(self, options: Option<&CodeOptions>) -> Result<()> {
        anyhow::ensure!(
            self == Self::CodeOrigin || options.is_none(),
            "code_options require capture_mode code-origin"
        );
        if let Some(options) = options {
            options.validate()?;
        }
        Ok(())
    }
    pub fn validate_runtime(self, runtime: &str, options: Option<&CodeOptions>) -> Result<()> {
        self.validate_options(options)?;
        anyhow::ensure!(
            self != Self::CodeOrigin || runtime == "node",
            "code-origin requires Node; the selected runtime is {runtime}"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureRequest {
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub artifact_dir: PathBuf,
    pub run_id: String,
    pub runtime: String,
    #[serde(default)]
    pub capture_mode: CaptureMode,
    #[serde(default)]
    pub code_options: Option<CodeOptions>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturePlan {
    pub command: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub diagnostics: Vec<String>,
    pub runtime_version: Option<String>,
    #[serde(default)]
    pub collector_identity: Option<serde_json::Value>,
    /// Adapter-generated transient values, keyed by their stable semantic role.
    #[serde(default)]
    pub session_identity: BTreeMap<String, String>,
    #[serde(default)]
    pub output: CaptureOutput,
}

/// What this adapter promises to finalize, rather than a claim about all target work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CaptureOutput {
    #[default]
    Unspecified,
    Profiles {
        format: String,
    },
    FunctionCounts {
        method: String,
        maximum_bytes: u64,
        maximum_functions: u32,
    },
    GraalCpuSampler,
    DenoWorkers {
        producer_id: String,
    },
    BunProducers {
        session_id: String,
        root_registration_id: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureArtifact {
    pub path: PathBuf,
    pub format: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawProfile {
    pub path: PathBuf,
    pub format: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureProducer {
    pub id: String,
    pub process_id: u64,
    pub runtime_id: String,
    pub profiles: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionState {
    Complete,
    Incomplete,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureCompletion {
    pub state: CompletionState,
    pub scope: String,
    /// None means the adapter did not establish a producer inventory.
    pub expected_producers: Option<Vec<String>>,
    pub observed_producers: Option<Vec<CaptureProducer>>,
    pub diagnostics: Vec<String>,
    pub limitations: Vec<String>,
}

impl CaptureCompletion {
    pub fn ensure_complete(&self) -> Result<()> {
        if matches!(self.state, CompletionState::Incomplete) {
            bail!("{}", self.diagnostics.join("; "));
        }
        Ok(())
    }
}

/// Raw evidence and collection conditions; no decoding or accepted observations.
#[derive(Debug, Clone, Serialize)]
pub struct RawCaptureBundle {
    pub artifacts: Vec<CaptureArtifact>,
    pub profiles: Vec<RawProfile>,
    pub completion: CaptureCompletion,
}

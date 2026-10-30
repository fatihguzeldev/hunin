//! Explicit requests for additional counting; ordinary recordings remain sampled.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FunctionSelector {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CountRequest {
    pub functions: Vec<FunctionSelector>,
    pub allow_rebuild: bool,
    /// An explicit compiler invocation; `{output}` names the isolated binary.
    pub build_command: Option<Vec<String>>,
    pub max_functions: u32,
    pub max_output_bytes: u64,
}

impl Default for CountRequest {
    fn default() -> Self {
        Self {
            functions: Vec::new(),
            allow_rebuild: false,
            build_command: None,
            max_functions: 8192,
            max_output_bytes: 8 * 1024 * 1024,
        }
    }
}

impl CountRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=8192).contains(&self.max_functions),
            "max_functions must be between 1 and 8192"
        );
        anyhow::ensure!(
            (1024..=8 * 1024 * 1024).contains(&self.max_output_bytes),
            "max_output_bytes must be between 1024 and 8388608"
        );
        anyhow::ensure!(
            self.functions.len() <= 256,
            "at most 256 function selectors are supported"
        );
        for function in &self.functions {
            anyhow::ensure!(
                !function.name.is_empty()
                    && function.name.len() <= 1024
                    && !function.name.contains('\0'),
                "function names must be nonempty, bounded and contain no NUL"
            );
            anyhow::ensure!(function.line != Some(0), "function lines start at 1");
            if let Some(file) = &function.file {
                anyhow::ensure!(
                    !file.is_empty() && file.len() <= 4096 && !file.contains('\0'),
                    "function files must be nonempty, bounded and contain no NUL"
                );
            }
        }
        if let Some(command) = &self.build_command {
            anyhow::ensure!(self.allow_rebuild, "build_command requires allow_rebuild");
            anyhow::ensure!(
                !command.is_empty()
                    && command.len() <= 256
                    && command
                        .iter()
                        .all(|arg| !arg.contains('\0') && arg.len() <= 16384),
                "build_command must contain bounded arguments without NUL"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CountPlan {
    pub status: String,
    pub method: Option<String>,
    pub reason: Option<String>,
    pub requested_scope: String,
    pub collection_scope: String,
    pub functions: Vec<FunctionSelector>,
    pub requires_rebuild: bool,
    pub limitations: Vec<String>,
}

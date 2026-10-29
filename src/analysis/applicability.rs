use crate::model::runs::{Artifact, Run, RunStatus};
use anyhow::{Context, Result, bail};
use serde_json::Value;

pub fn validate_declaration(requirements: &Value) -> Result<()> {
    let object = requirements
        .as_object()
        .context("requirements must be an object")?;
    for key in object.keys() {
        if ![
            "runtime",
            "source_fingerprint",
            "source_independent",
            "artifact_digests",
            "executable_sha256",
            "runtime_version",
        ]
        .contains(&key.as_str())
        {
            bail!("unknown applicability requirement: {key}");
        }
    }
    if requirements["runtime"].as_str().is_none_or(str::is_empty) {
        bail!(
            "applicability runtime is required (use explicit '*' for runtime-independent analysis)"
        );
    }
    if requirements["source_independent"] != true
        && requirements["source_fingerprint"]
            .as_str()
            .is_none_or(|s| s.is_empty() || s == "unknown")
    {
        bail!(
            "known source_fingerprint or explicit source_independent:true is required; unknown applicability cannot be reused"
        );
    }
    if requirements
        .get("source_independent")
        .is_some_and(|value| !value.is_boolean())
    {
        bail!("source_independent must be a boolean declaration");
    }
    for key in ["executable_sha256", "runtime_version"] {
        if requirements.get(key).is_some_and(|value| {
            value
                .as_str()
                .is_none_or(|s| s.is_empty() || s == "unknown")
        }) {
            bail!("known {key} is required when declared");
        }
    }
    if let Some(values) = requirements.get("artifact_digests") {
        if values.as_array().is_none_or(|list| {
            list.iter().any(|value| {
                value
                    .as_str()
                    .is_none_or(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
            })
        }) {
            bail!("artifact_digests must contain SHA-256 digests");
        }
    }
    Ok(())
}
pub fn check(requirements: &Value, run: &Run, artifacts: &[Artifact]) -> Result<()> {
    validate_declaration(requirements)?;
    if run.status == RunStatus::Running {
        bail!("analysis requires a finished input recording");
    }
    let runtime = requirements["runtime"].as_str().unwrap();
    if runtime != "*" && runtime != run.runtime {
        bail!(
            "analyzer runtime applicability mismatch: requires {runtime}, recording is {}",
            run.runtime
        );
    }
    if let Some(expected) = requirements["source_fingerprint"].as_str() {
        if expected != run.source["fingerprint"].as_str().unwrap_or("unknown") {
            bail!(
                "analyzer source/build applicability mismatch; register against the measured source identity"
            );
        }
    }
    for key in ["executable_sha256", "runtime_version"] {
        if let Some(expected) = requirements.get(key) {
            if expected
                .as_str()
                .is_none_or(|s| s.is_empty() || s == "unknown")
            {
                bail!("known {key} is required when declared");
            }
            if expected != &run.source[key] {
                bail!("analyzer {key} applicability mismatch or unavailable identity");
            }
        }
    }
    if let Some(required) = requirements["artifact_digests"].as_array() {
        for digest in required {
            if !artifacts.iter().any(|artifact| {
                artifact
                    .sha256
                    .eq_ignore_ascii_case(digest.as_str().unwrap())
            }) {
                bail!("required input artifact digest is unavailable: {digest}");
            }
        }
    }
    Ok(())
}

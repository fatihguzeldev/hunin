//! Validate the analyzer JSONL boundary against its selected input observations.
use super::{files, input::Input, process};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn validate_contract(value: Option<&Value>) -> Result<()> {
    if let Some(value) = value {
        let declaration = value
            .as_object()
            .context("output_contract must be an object")?;
        anyhow::ensure!(
            declaration.len() == 2 && value["name"] == "code-mapping" && value["version"] == 1,
            "unsupported analyzer output_contract; expected code-mapping version 1"
        );
    }
    Ok(())
}

pub(super) fn validate(
    analysis: &Value,
    input: &Input,
    output: &Path,
    control: &process::Control,
) -> Result<Vec<Value>> {
    validate_contract(analysis.get("output_contract"))?;
    control.check()?;
    let bytes =
        files::read(output, process::OUTPUT_LIMIT, || control.check()).map_err(|error| {
            if error.downcast_ref::<files::ByteLimitExceeded>().is_some() {
                error.context("analyzer output exceeds the 16 MiB output limit")
            } else {
                error
            }
        })?;
    let text = String::from_utf8(bytes).context("analyzer output must be UTF-8 JSONL")?;
    control.check()?;
    let inputs = &input.record_ids;
    let mut records = Vec::new();
    for (line, value) in text
        .lines()
        .enumerate()
        .filter(|(_, text)| !text.trim().is_empty())
    {
        control.check()?;
        let mut record: Value = serde_json::from_str(value)
            .with_context(|| format!("invalid analyzer JSONL at line {}", line + 1))?;
        if !record.is_object() || record["basis"] != "derived" {
            bail!(
                "analyzer observations must retain derived basis (line {})",
                line + 1
            );
        }
        let evidence = record["evidence"]
            .as_array()
            .filter(|v| !v.is_empty())
            .context("every derived observation needs input record evidence")?;
        for reference in evidence {
            if !reference.as_str().is_some_and(|id| inputs.contains(id)) {
                bail!("derived evidence must reference an input observation from this run");
            }
        }
        if let Some(code) = crate::model::code::parse_record(&record)? {
            for reference in code.artifact_refs() {
                let artifact = input
                    .artifacts
                    .get(&reference.artifact_id)
                    .context("code proof artifact is not part of the selected analyzer input")?;
                anyhow::ensure!(
                    artifact.sha256 == reference.sha256,
                    "code proof artifact digest disagrees with the selected analyzer input"
                );
                if let (Some(offset), Some(length)) = (reference.offset, reference.length) {
                    anyhow::ensure!(
                        offset
                            .checked_add(length)
                            .is_some_and(|end| end <= artifact.size),
                        "code proof byte range exceeds the selected artifact"
                    );
                }
            }
            if let crate::model::code::CodeRecord::Mapping(mapping) = code {
                for reference in &mapping.proof.semantic_evidence {
                    anyhow::ensure!(
                        input.record_ids.contains(reference),
                        "semantic mapping evidence must reference the selected analyzer input"
                    );
                }
            }
        }
        if record.get("attributes").is_none() {
            record["attributes"] = json!({});
        }
        if !record["attributes"].is_object() {
            bail!("analyzer attributes must be an object");
        }
        record["attributes"]["hunin_analysis_id"] = analysis["id"].clone();
        record["attributes"]["analysis_package_digest"] = analysis["package_digest"].clone();
        records.push(record);
    }
    control.check()?;
    Ok(records)
}

#[cfg(test)]
mod tests;

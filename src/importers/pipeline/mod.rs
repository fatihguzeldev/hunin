//! Normalize one immutable input snapshot under one shared output budget.
mod capture;
mod decode;
#[cfg(test)]
use crate::importers::output::{self, NormalizationOutputLimit, OutputDimension};
use crate::importers::{ImportedProfile, formats::ProfileFormat, inputs, output::OutputBudget};
use anyhow::Result;
use capture::capture_inputs;
use decode::decode_snapshot;
use std::path::Path;

pub fn import(path: &Path, format: &str, run_id: &str) -> Result<ImportedProfile> {
    import_controlled(path, format, run_id, None)
}
pub fn import_with_native_context(
    path: &Path,
    format: &str,
    run_id: &str,
    manifest_path: &Path,
) -> Result<ImportedProfile> {
    import_with_native_context_controlled(path, format, run_id, manifest_path, None)
}

pub(crate) fn import_with_native_context_controlled(
    path: &Path,
    format: &str,
    run_id: &str,
    manifest_path: &Path,
    deadline: Option<std::time::Instant>,
) -> Result<ImportedProfile> {
    anyhow::ensure!(
        ProfileFormat::parse(format)? == ProfileFormat::Firefox,
        "native image context requires a Firefox/samply recording"
    );
    normalize(
        path,
        format,
        run_id,
        Some(manifest_path),
        inputs::Limits::default(),
        &mut OutputBudget::default(),
        deadline,
    )
}

pub(crate) fn import_controlled(
    path: &Path,
    format: &str,
    run_id: &str,
    deadline: Option<std::time::Instant>,
) -> Result<ImportedProfile> {
    normalize(
        path,
        format,
        run_id,
        None,
        inputs::Limits::default(),
        &mut OutputBudget::default(),
        deadline,
    )
}

pub(crate) fn import_with_budget_controlled(
    path: &Path,
    format: &str,
    run_id: &str,
    budget: &mut OutputBudget,
    deadline: Option<std::time::Instant>,
) -> Result<ImportedProfile> {
    normalize(
        path,
        format,
        run_id,
        None,
        inputs::Limits::default(),
        budget,
        deadline,
    )
}

fn normalize(
    path: &Path,
    format: &str,
    run_id: &str,
    native_context: Option<&Path>,
    limits: inputs::Limits,
    budget: &mut OutputBudget,
    deadline: Option<std::time::Instant>,
) -> Result<ImportedProfile> {
    budget.set_deadline(deadline);
    let deadline = budget.deadline();
    budget.check()?;
    let (snapshot, digest) = capture_inputs(path, format, limits, deadline)?;
    decode_snapshot(
        path,
        format,
        run_id,
        native_context,
        &snapshot,
        digest,
        budget,
    )
}

#[cfg(test)]
mod input_tests;
#[cfg(test)]
mod output_tests;

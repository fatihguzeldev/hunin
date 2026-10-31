//! Verify retained package identity and copy the registered source for an attempt.
use super::{
    MAX_BYTES, MAX_FILES,
    identity::{executable, source_file},
    source_read_error,
};
use crate::analysis::files;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(in crate::analysis) fn verify(
    analysis: &Value,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<PathBuf> {
    checkpoint()?;
    crate::analysis::output::validate_contract(analysis.get("output_contract"))?;
    let source = PathBuf::from(
        analysis["package_dir"]
            .as_str()
            .context("managed package path missing")?,
    )
    .join("source");
    let entries = analysis["files"]
        .as_object()
        .context("package file manifest missing")?;
    if entries.len() > MAX_FILES {
        bail!("analyzer package exceeds {MAX_FILES} files");
    }
    let mut total = 0;
    for (path, identity) in entries {
        checkpoint()?;
        let target = source_file(&source, path)?;
        let (sha256, bytes) = files::hash_bounded(&target, MAX_BYTES - total, &mut checkpoint)
            .map_err(source_read_error)?;
        total += bytes;
        if sha256 != identity["sha256"] {
            bail!(
                "managed analyzer package changed; register its new version before reuse: {path}"
            );
        }
    }
    let program = analysis["executable"]["path"]
        .as_str()
        .context("analyzer executable identity missing")?;
    let program = if analysis["executable"]["packaged"] == true {
        executable(analysis["command"][0].as_str().unwrap(), &source)?
    } else {
        PathBuf::from(program)
    };
    if files::hash(&program, &mut checkpoint)
        .context("analyzer executable prerequisite unavailable")?
        != analysis["executable"]["sha256"]
    {
        bail!("analyzer executable version changed; register against the new runtime before reuse");
    }
    checkpoint()?;
    Ok(source)
}

pub(in crate::analysis) fn copy_workspace(
    analysis: &Value,
    source: &Path,
    workspace: &Path,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<()> {
    checkpoint()?;
    fs::create_dir_all(workspace)?;
    let entries = analysis["files"]
        .as_object()
        .context("package file manifest missing")?;
    if entries.len() > MAX_FILES {
        bail!("analyzer package exceeds {MAX_FILES} files");
    }
    let mut total = 0;
    for (path, identity) in entries {
        checkpoint()?;
        let source = source_file(source, path)?;
        let destination = workspace.join(path);
        fs::create_dir_all(destination.parent().unwrap())?;
        let (sha256, bytes) =
            files::copy(&source, &destination, MAX_BYTES - total, &mut checkpoint)
                .map_err(source_read_error)?;
        total += bytes;
        if sha256 != identity["sha256"] {
            bail!("managed package changed while preparing execution");
        }
        fs::set_permissions(destination, fs::metadata(&source)?.permissions())?;
    }
    checkpoint()?;
    Ok(())
}

#[cfg(test)]
mod tests;

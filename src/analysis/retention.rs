//! Named analyzer evidence, with actual staging bytes and caller-owned checks.
use crate::storage::Store;
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Serialize)]
pub(super) struct Limits {
    pub entries: usize,
    pub files: usize,
    pub bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            entries: 8000,
            files: 2000,
            bytes: 272 * 1024 * 1024,
        }
    }
}

#[derive(Serialize)]
struct Report {
    policy: &'static str,
    limits: Limits,
    visited_entries: usize,
    selected_files: usize,
    actual_read_bytes: u64,
    attempt_metadata_bytes: u64,
    recorded_artifact_references: usize,
    complete: bool,
    original_workspace_preserved: bool,
    diagnostic: Option<String>,
}

struct PendingMetadata(Option<PathBuf>);
impl Drop for PendingMetadata {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}
struct MetadataWriter<'a> {
    file: fs::File,
    maximum: u64,
    written: &'a mut u64,
    checkpoint: &'a mut dyn FnMut() -> Result<()>,
}
impl Write for MetadataWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for chunk in bytes.chunks(64 * 1024) {
            (self.checkpoint)().map_err(io::Error::other)?;
            if chunk.len() as u64 > self.maximum.saturating_sub(*self.written) {
                return Err(io::Error::other(
                    "analyzer attempt metadata exceeds raw retention byte budget",
                ));
            }
            self.file.write_all(chunk)?;
            *self.written += chunk.len() as u64;
            (self.checkpoint)().map_err(io::Error::other)?;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
fn write_attempt(
    document: &Value,
    directory: &Path,
    maximum: u64,
    written: &mut u64,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    checkpoint()?;
    let path = directory.join(format!(".attempt-{}.pending", uuid::Uuid::new_v4()));
    let mut pending = PendingMetadata(Some(path.clone()));
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let mut output = MetadataWriter {
        file,
        maximum,
        written,
        checkpoint,
    };
    {
        // Pretty serialization emits many small fragments. Buffer them above
        // the checked writer without building the whole document in memory.
        let mut buffered = io::BufWriter::with_capacity(64 * 1024, &mut output);
        serde_json::to_writer_pretty(&mut buffered, document)?;
        buffered.flush()?;
    }
    (output.checkpoint)()?;
    output.file.sync_all()?;
    (output.checkpoint)()?;
    drop(output);
    fs::rename(path, directory.join("attempt.json"))?;
    pending.0.take();
    checkpoint()?;
    Ok(())
}

pub(super) fn retain(
    store: &Store,
    document: &mut Value,
    directory: &Path,
    limits: Limits,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<()> {
    let mut report = Report {
        policy: "analysis-raw-retention-v1",
        limits,
        visited_entries: 0,
        selected_files: 0,
        actual_read_bytes: 0,
        attempt_metadata_bytes: 0,
        recorded_artifact_references: 0,
        complete: false,
        original_workspace_preserved: true,
        diagnostic: None,
    };
    let mut artifacts = Vec::new();
    let result = (|| {
        write_attempt(
            document,
            directory,
            limits.bytes,
            &mut report.attempt_metadata_bytes,
            &mut checkpoint,
        )?;
        for entry in fs::read_dir(directory)? {
            checkpoint()?;
            let path = entry?.path();
            report.visited_entries = report.visited_entries.saturating_add(1);
            anyhow::ensure!(
                report.visited_entries <= limits.entries,
                "analyzer raw retention exceeds {} directory entries",
                limits.entries
            );
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if ![
                "input.json",
                "stdout.jsonl",
                "stderr.txt",
                "dependencies.json",
                "attempt.json",
            ]
            .contains(&name)
                && !(name.starts_with("setup-")
                    && (name.ends_with(".stdout") || name.ends_with(".stderr")))
            {
                continue;
            }
            anyhow::ensure!(
                fs::symlink_metadata(&path)?.is_file(),
                "analyzer evidence is not a regular file: {}",
                path.display()
            );
            report.selected_files = report.selected_files.saturating_add(1);
            anyhow::ensure!(
                report.selected_files <= limits.files,
                "analyzer raw retention exceeds {} files",
                limits.files
            );
            let format = if name == "input.json" {
                "analysis-input"
            } else {
                "analysis-output"
            };
            let remaining = limits.bytes.saturating_sub(report.actual_read_bytes);
            let artifact = store.add_artifact_controlled(
                document["run_id"]
                    .as_str()
                    .context("analyzer evidence run missing")?,
                &path,
                format,
                remaining,
                &mut report.actual_read_bytes,
                &mut checkpoint,
            )?;
            artifacts.push(serde_json::to_value(artifact)?);
            checkpoint()?;
        }
        checkpoint()
    })();
    report.recorded_artifact_references = artifacts.len();
    report.complete = result.is_ok();
    report.diagnostic = result.as_ref().err().map(|error| format!("{error:#}"));
    if result.is_err()
        && let Some(previous) = document["raw_artifacts"].as_array_mut()
    {
        // A failed recovery pass must not erase references already retained
        // before the interruption. These remain partial evidence, not a result.
        previous.extend(artifacts);
    } else {
        document["raw_artifacts"] = json!(artifacts);
    }
    document["raw_retention"] = serde_json::to_value(report)?;
    result
}

#[cfg(test)]
#[path = "retention_tests.rs"]
mod tests;

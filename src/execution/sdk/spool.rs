//! Discover and retain every selected spool before bounded immutable decoding.
use super::contract::{BudgetReport, RawRetention, SpoolSnapshot};
use crate::storage::Store;
use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, Read, Seek, SeekFrom},
    path::Path,
    time::Instant,
};

#[derive(Debug)]
pub(super) struct DeadlineElapsed;
impl std::fmt::Display for DeadlineElapsed {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str("SDK finishing checkpoint deadline elapsed")
    }
}
impl std::error::Error for DeadlineElapsed {}

fn checkpoint(deadline: Option<Instant>) -> Result<()> {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(DeadlineElapsed.into());
    }
    Ok(())
}

pub(super) fn read(
    store: &Store,
    run_id: &str,
    directory: &Path,
    snapshot: &mut SpoolSnapshot,
    budget: &mut BudgetReport,
    raw_retention: &mut RawRetention,
    deadline: Option<Instant>,
) -> Result<Vec<Value>> {
    checkpoint(deadline)?;
    let limits = budget.limits;
    let mut records = Vec::new();
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        checkpoint(deadline)?;
        budget.work.directory_entries += 1;
        budget.check(
            "directory_entries",
            budget.work.directory_entries as u64,
            limits.max_directory_entries as u64,
        )?;
        let path = entry?.path();
        if !path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            continue;
        }
        budget.work.spool_files += 1;
        budget.check(
            "spool_files",
            budget.work.spool_files as u64,
            limits.max_spool_files as u64,
        )?;
        paths.push(path);
    }
    paths.sort();
    let mut retained = Vec::new();
    for path in paths {
        checkpoint(deadline)?;
        let maximum = raw_retention.maximum_file_bytes.min(
            raw_retention
                .maximum_read_bytes
                .saturating_sub(raw_retention.read_bytes),
        );
        let artifact = store.add_artifact_controlled(
            run_id,
            &path,
            "hunin-jsonl",
            maximum,
            &mut raw_retention.read_bytes,
            || checkpoint(deadline),
        )?;
        retained.push(artifact);
        snapshot.retained_artifact_count += 1;
    }
    // Snapshot every discovered spool before decoding any of them. A rejected
    // prefix must not hide later raw evidence or imply a complete inventory.
    snapshot.inventory_retained = true;
    for artifact in &retained {
        checkpoint(deadline)?;
        let mut file = fs::File::open(store.artifact_path(run_id, &artifact.id)?)?;
        if artifact.size > 0 {
            snapshot.nonempty_artifact_count += 1;
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0u8; 1];
            file.read_exact(&mut last)?;
            budget.work.read_bytes += 1;
            checkpoint(deadline)?;
            budget.check("read_bytes", budget.work.read_bytes, limits.max_read_bytes)?;
            if last[0] != b'\n' {
                snapshot.nonempty_without_final_lf_count += 1;
            }
        }
        snapshot.byte_termination_checked_artifact_count += 1;
    }
    for artifact in retained {
        checkpoint(deadline)?;
        let retained = store.artifact_path(run_id, &artifact.id)?;
        let before_file = budget.work.read_bytes;
        // One sentinel byte distinguishes exact EOF from a budget overrun.
        // Take bounds actual reads, even when metadata or a line is misleading.
        let allowance = limits
            .max_read_bytes
            .saturating_sub(before_file)
            .saturating_add(1);
        let mut reader = std::io::BufReader::new(fs::File::open(retained)?.take(allowance));
        let mut line = Vec::new();
        let mut index = 0;
        loop {
            checkpoint(deadline)?;
            line.clear();
            let result = Read::take(&mut reader, limits.max_line_bytes.saturating_add(1))
                .read_until(b'\n', &mut line);
            // Include BufReader prefetch in actual work, not only parsed bytes.
            budget.work.read_bytes =
                before_file.saturating_add(allowance - reader.get_ref().limit());
            budget.check("read_bytes", budget.work.read_bytes, limits.max_read_bytes)?;
            let count = result.with_context(|| {
                format!("read SDK artifact {} line {}", artifact.name, index + 1)
            })?;
            checkpoint(deadline)?;
            budget.work.max_physical_line_bytes =
                budget.work.max_physical_line_bytes.max(count as u64);
            budget.check("line_bytes", count as u64, limits.max_line_bytes)?;
            if count == 0 {
                break;
            }
            index += 1;
            let line = std::str::from_utf8(&line).with_context(|| {
                format!("SDK artifact {} line {} is not UTF-8", artifact.name, index)
            })?;
            if !line.trim().is_empty() {
                budget.work.nonblank_records += 1;
                budget.check(
                    "records",
                    budget.work.nonblank_records as u64,
                    limits.max_records as u64,
                )?;
                records.push(
                    serde_json::from_str(line).with_context(|| {
                        format!("SDK artifact {} line {}", artifact.name, index)
                    })?,
                );
                checkpoint(deadline)?;
            }
        }
    }
    checkpoint(deadline)?;
    Ok(records)
}

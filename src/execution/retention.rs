//! Exact raw artifact preservation with bounded discovery and source reads.
use crate::{
    collection::{self, CaptureArtifact},
    host,
    storage::Store,
};
use anyhow::{Result, bail};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Serialize)]
pub(super) struct Limits {
    pub directory_entries: usize,
    pub regular_files: usize,
    pub directory_depth: usize,
    pub file_bytes: u64,
    pub total_source_bytes: u64,
    pub wall_time_ms: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            directory_entries: 8192,
            regular_files: 4096,
            directory_depth: 64,
            file_bytes: 512 * 1024 * 1024,
            total_source_bytes: 1024 * 1024 * 1024,
            wall_time_ms: 120_000,
        }
    }
}

#[derive(Serialize)]
pub(super) struct Failure {
    stage: &'static str,
    reason: &'static str,
    diagnostic: String,
    maximum_bytes: Option<u64>,
    observed_bytes: Option<u64>,
}

#[derive(Serialize)]
pub(super) struct Report {
    name: &'static str,
    pub complete: bool,
    inventory_complete: bool,
    limits: Limits,
    entries_seen: usize,
    regular_files_seen: usize,
    nonregular_entries: usize,
    maximum_depth_seen: usize,
    files_retained: usize,
    source_read_bytes: u64,
    retained_bytes: u64,
    original_workspace: PathBuf,
    original_workspace_removed_by_retention: bool,
    failure: Option<Failure>,
    scope: &'static str,
    limitations: &'static str,
}

pub(super) struct Preserved {
    pub artifacts: Vec<CaptureArtifact>,
    pub report: Report,
}

pub(super) fn preserve(store: &Store, run_id: &str, path: &Path, limits: Limits) -> Preserved {
    let began = Instant::now();
    let mut checkpoint = || -> Result<()> {
        if began.elapsed() >= Duration::from_millis(limits.wall_time_ms) {
            bail!("raw capture retention checkpoint deadline elapsed");
        }
        Ok(())
    };
    let inventory = host::inventory(
        path,
        &host::InventoryLimits {
            entries: limits.directory_entries,
            files: limits.regular_files,
            depth: limits.directory_depth,
        },
        &mut checkpoint,
    );
    let artifacts = collection::artifacts_from_paths(inventory.files);
    let failure = inventory.failure.map(|failure| Failure {
        stage: "inventory",
        reason: failure.kind,
        diagnostic: failure.diagnostic,
        maximum_bytes: None,
        observed_bytes: None,
    });
    let mut report = Report {
        name: "capture-raw-retention-v1",
        complete: false,
        inventory_complete: inventory.complete,
        limits,
        entries_seen: inventory.entries,
        regular_files_seen: inventory.regular_files,
        nonregular_entries: inventory.nonregular_entries,
        maximum_depth_seen: inventory.maximum_depth,
        files_retained: 0,
        source_read_bytes: 0,
        retained_bytes: 0,
        original_workspace: path.into(),
        original_workspace_removed_by_retention: false,
        failure,
        scope: "regular files in the selected capture inventory; input bytes read for staging",
        limitations: "Safety policy, not recording capacity or a whole-finishing deadline. One extra source byte may be read to detect growth. Static nonregular entries are omitted. Source-read counters exclude destination dedup verification, which reads at most twice each fully staged file size. Checkpoints cannot preempt kernel I/O, fsync or a database commit. Retention does not remove original capture files; their external stability is not guaranteed. No adversarial filesystem replacement or producer shutdown guarantee.",
    };
    for artifact in &artifacts {
        let maximum = limits.file_bytes.min(
            limits
                .total_source_bytes
                .saturating_sub(report.source_read_bytes),
        );
        match store.add_artifact_controlled(
            run_id,
            &artifact.path,
            &artifact.format,
            maximum,
            &mut report.source_read_bytes,
            &mut checkpoint,
        ) {
            Ok(retained) => {
                report.files_retained += 1;
                report.retained_bytes += retained.size;
            }
            Err(error) => {
                let exceeded = error.downcast_ref::<crate::storage::ArtifactByteLimit>();
                if report.failure.is_none() {
                    report.failure = Some(Failure {
                        stage: "staging",
                        reason: if exceeded.is_some() {
                            "source_bytes"
                        } else if began.elapsed() >= Duration::from_millis(limits.wall_time_ms) {
                            "checkpoint"
                        } else {
                            "io"
                        },
                        diagnostic: format!("{error:#}"),
                        maximum_bytes: exceeded.map(|limit| limit.maximum),
                        observed_bytes: exceeded.map(|limit| limit.observed),
                    });
                }
                break;
            }
        }
    }
    report.complete = report.failure.is_none();
    Preserved { artifacts, report }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn store(root: &Path) -> Store {
        let store = Store::open(root).unwrap();
        store.save_run(&serde_json::from_value(json!({"id":"capture-budget","name":"capture","status":"completed","command":[],"cwd":".","runtime":"none","started_at_us":1,"finished_at_us":2,"exit_code":0,"diagnostics":[],"source":{},"correctness":{}})).unwrap()).unwrap();
        store
    }
    fn limits() -> Limits {
        Limits {
            file_bytes: 4,
            total_source_bytes: 8,
            ..Limits::default()
        }
    }

    #[test]
    fn exact_raw_limits_preserve_each_file_once_and_overrun_keeps_original_evidence() {
        let root = tempfile::tempdir().unwrap();
        let inputs = tempfile::tempdir().unwrap();
        let store = store(root.path());
        fs::write(inputs.path().join("a.bin"), b"1234").unwrap();
        fs::write(inputs.path().join("b.bin"), b"5678").unwrap();
        let exact = preserve(&store, "capture-budget", inputs.path(), limits());
        assert!(exact.report.complete);
        assert_eq!(exact.report.source_read_bytes, 8);
        assert_eq!(exact.report.files_retained, 2);
        assert_eq!(exact.report.retained_bytes, 8);
        fs::write(inputs.path().join("c.bin"), b"too big").unwrap();
        let over = preserve(&store, "capture-budget", inputs.path(), limits());
        assert!(!over.report.complete);
        assert!(over.report.inventory_complete);
        assert_eq!(over.report.files_retained, 2);
        assert_eq!(over.report.source_read_bytes, 8);
        assert_eq!(fs::read(inputs.path().join("c.bin")).unwrap(), b"too big");
        assert_eq!(store.artifacts("capture-budget").unwrap().len(), 2);
        assert_eq!(over.report.failure.unwrap().reason, "source_bytes");
    }

    #[test]
    fn incomplete_inventory_and_expired_budget_cannot_claim_complete_retention() {
        let root = tempfile::tempdir().unwrap();
        let inputs = tempfile::tempdir().unwrap();
        let store = store(root.path());
        fs::write(inputs.path().join("a.bin"), b"1234").unwrap();
        fs::write(inputs.path().join("b.bin"), b"5678").unwrap();
        let limited = preserve(
            &store,
            "capture-budget",
            inputs.path(),
            Limits {
                regular_files: 1,
                ..limits()
            },
        );
        assert!(!limited.report.complete);
        assert!(!limited.report.inventory_complete);
        assert_eq!(limited.report.regular_files_seen, 2);
        assert_eq!(limited.report.failure.unwrap().reason, "files");
        let expired = preserve(
            &store,
            "capture-budget",
            inputs.path(),
            Limits {
                wall_time_ms: 0,
                ..limits()
            },
        );
        assert!(!expired.report.complete);
        assert_eq!(expired.report.source_read_bytes, 0);
        assert_eq!(expired.report.files_retained, 0);
        assert_eq!(expired.report.failure.unwrap().reason, "checkpoint");
    }
}

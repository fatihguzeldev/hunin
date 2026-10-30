use super::*;
use crate::model::runs::{Artifact, Run, RunStatus};
use rusqlite::{Connection, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;

fn fixture() -> (tempfile::TempDir, Store, Artifact) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store
        .save_run(&Run {
            id: "recording".into(),
            name: "inspection control".into(),
            status: RunStatus::Completed,
            command: vec![],
            cwd: "/removed".into(),
            runtime: "fixture".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: Some(0),
            diagnostics: vec![],
            source: json!({}),
            correctness: json!({}),
        })
        .unwrap();
    let artifact = store
        .add_artifact_bytes("recording", "retained.raw", "raw", b"retained bytes")
        .unwrap();
    (root, store, artifact)
}

#[test]
fn copied_store_inspection_distinguishes_missing_unreferenced_and_unknown_without_writes() {
    let (original, _store, artifact) = fixture();
    let copy = tempfile::tempdir().unwrap();
    Connection::open(original.path().join("hunin.sqlite"))
        .unwrap()
        .execute(
            "VACUUM INTO ?1",
            [copy.path().join("hunin.sqlite").to_str().unwrap()],
        )
        .unwrap();
    fs::create_dir(copy.path().join("artifacts")).unwrap();
    let orphan = format!("{:x}", Sha256::digest(b"unreferenced bytes"));
    fs::write(
        copy.path().join("artifacts").join(&orphan),
        b"unreferenced bytes",
    )
    .unwrap();
    let pending = copy.path().join("artifacts/.pending-unknown-owner");
    fs::write(&pending, b"unfinished").unwrap();
    let workspace = copy.path().join("captures/old-workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("evidence.raw"), b"do not sweep").unwrap();
    // This test-only copy intentionally bypasses Store::open: inspect must
    // neither bootstrap nor migrate the copied database.
    let copied = Store {
        root: copy.path().canonicalize().unwrap(),
        identity: super::super::ExpectedIdentity::Any,
    };
    let before = fs::read(copy.path().join("hunin.sqlite")).unwrap();
    let report = copied.inspect(InspectionLimits::default()).unwrap();
    assert!(report.findings.iter().any(|finding| matches!(finding,
        InspectionFinding::MissingReferencedBlob { sha256, .. } if sha256 == &artifact.sha256)));
    assert!(report.findings.iter().any(|finding| matches!(finding,
        InspectionFinding::UnreferencedAtSnapshot { sha256, .. } if sha256 == &orphan)));
    assert!(report.findings.iter().any(|finding| matches!(finding,
        InspectionFinding::PendingOwnershipUnknown { relative_path, .. } if relative_path == &PathBuf::from("artifacts/.pending-unknown-owner"))));
    assert!(report.findings.iter().any(|finding| matches!(
        finding,
        InspectionFinding::WorkspaceOwnershipUnknown {
            metadata_reference: WorkspaceMetadataReference::Absent,
            ..
        }
    )));
    assert_eq!(report.content_hashes_checked, 0);
    assert!(
        report
            .scans
            .iter()
            .all(|scan| scan.status == InspectionScanStatus::Exhausted)
    );
    assert_eq!(fs::read(copy.path().join("hunin.sqlite")).unwrap(), before);
    assert_eq!(fs::read(&pending).unwrap(), b"unfinished");
    assert_eq!(
        fs::read(workspace.join("evidence.raw")).unwrap(),
        b"do not sweep"
    );
    assert!(
        original
            .path()
            .join("artifacts")
            .join(artifact.sha256)
            .is_file()
    );
}

#[test]
fn blob_before_reference_commit_is_not_treated_as_deletion_authority() {
    let (_root, store, _artifact) = fixture();
    let bytes = b"live publisher complete bytes";
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let path = store.root().join("artifacts").join(&sha256);
    let mut writer = Connection::open(store.root().join("hunin.sqlite")).unwrap();
    let transaction = writer
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    fs::write(&path, bytes).unwrap();
    transaction
        .execute(
            "INSERT INTO artifacts(run_id,id,metadata,sha256) VALUES(?1,?2,?3,?4)",
            params![
                "recording",
                "live-reference",
                "not materialized by inspection",
                sha256
            ],
        )
        .unwrap();
    let report = store.inspect(InspectionLimits::default()).unwrap();
    assert!(
        report.findings.iter().any(|finding| matches!(finding,
        InspectionFinding::UnreferencedAtSnapshot { sha256: observed, .. } if observed == &sha256))
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    transaction.commit().unwrap();
    let after = store.inspect(InspectionLimits::default()).unwrap();
    assert!(
        !after.findings.iter().any(|finding| matches!(finding,
        InspectionFinding::UnreferencedAtSnapshot { sha256: observed, .. } if observed == &sha256))
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn publication_rollback_leaves_observed_bytes_without_authorizing_cleanup() {
    let (_root, store, _) = fixture();
    let bytes = b"complete blob before crashed reference commit";
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let path = store.root().join("artifacts").join(&sha256);
    let mut writer = Connection::open(store.root().join("hunin.sqlite")).unwrap();
    let transaction = writer
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    fs::write(&path, bytes).unwrap();
    transaction
        .execute(
            "INSERT INTO artifacts(run_id,id,metadata,sha256) VALUES(?1,?2,?3,?4)",
            params!["recording", "interrupted-reference", "{}", sha256],
        )
        .unwrap();
    // Rollback models the durable state after interruption between the
    // complete-file rename and metadata commit, without killing a process.
    transaction.rollback().unwrap();
    let report = store.inspect(InspectionLimits::default()).unwrap();
    assert!(
        report.findings.iter().any(|finding| matches!(finding,
        InspectionFinding::UnreferencedAtSnapshot { sha256: observed, .. } if observed == &sha256))
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(report.content_hashes_checked, 0);
}

#[cfg(unix)]
#[test]
fn a_live_stream_pending_file_keeps_unknown_ownership_and_is_not_changed() {
    use std::io::Write;

    let (_root, store, _) = fixture();
    let source = store.root().join("stream.raw");
    let name = std::ffi::CString::new(source.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let other = store.clone();
    let import = std::thread::spawn(move || other.add_artifact("recording", &source, "raw"));
    let mut writer = fs::File::create(store.root().join("stream.raw")).unwrap();
    writer.write_all(b"still writing").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let pending = loop {
        let pending = fs::read_dir(store.root().join("artifacts"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .find(|entry| entry.file_name().to_string_lossy().starts_with(".pending-"));
        if let Some(entry) = pending {
            break entry.path();
        }
        assert!(
            Instant::now() < deadline,
            "live stream did not stage a file"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    let report = store.inspect(InspectionLimits::default()).unwrap();
    let relative = pending.strip_prefix(store.root()).unwrap();
    assert!(report.findings.iter().any(|finding| matches!(finding,
        InspectionFinding::PendingOwnershipUnknown { relative_path, .. } if relative_path == relative)));
    assert!(pending.is_file());
    drop(writer);
    let artifact = import.join().unwrap().unwrap();
    assert_eq!(
        fs::read(store.artifact_path("recording", &artifact.id).unwrap()).unwrap(),
        b"still writing"
    );
}

#[cfg(unix)]
#[test]
fn a_linked_artifact_directory_is_reported_without_following_it() {
    let (_root, store, _) = fixture();
    let outside = tempfile::tempdir().unwrap();
    let sha256 = format!("{:x}", Sha256::digest(b"outside bytes"));
    let foreign_blob = outside.path().join(sha256);
    fs::write(&foreign_blob, b"outside bytes").unwrap();
    fs::rename(
        store.root().join("artifacts"),
        store.root().join("retained-artifacts"),
    )
    .unwrap();
    std::os::unix::fs::symlink(outside.path(), store.root().join("artifacts")).unwrap();
    let report = store.inspect(InspectionLimits::default()).unwrap();
    for area in [
        InspectionArea::ArtifactReferences,
        InspectionArea::ArtifactFiles,
    ] {
        assert!(report.scans.iter().any(|scan| scan.area == area
            && scan.status == InspectionScanStatus::Unavailable
            && scan.examined == 0));
    }
    assert!(report.findings.iter().any(|finding| matches!(finding,
        InspectionFinding::UnfollowedPath { relative_path, kind: InspectionPathKind::Symlink }
            if relative_path == Path::new("artifacts"))));
    assert!(!report.findings.iter().any(|finding| matches!(
        finding,
        InspectionFinding::MissingReferencedBlob { .. }
            | InspectionFinding::UnreferencedAtSnapshot { .. }
    )));
    assert_eq!(fs::read(foreign_blob).unwrap(), b"outside bytes");
}

#[test]
fn row_entry_finding_and_time_limits_report_partial_work() {
    let (_root, store, _) = fixture();
    store
        .add_artifact_bytes("recording", "another.raw", "raw", b"another")
        .unwrap();
    let limits = InspectionLimits {
        max_artifact_rows: 1,
        max_filesystem_entries: 1,
        ..Default::default()
    };
    let report = store.inspect(limits).unwrap();
    assert!(report.limits_hit.contains(&InspectionLimit::ArtifactRows));
    assert!(
        report
            .limits_hit
            .contains(&InspectionLimit::FilesystemEntries)
    );
    assert!(
        report
            .scans
            .iter()
            .any(|scan| scan.status != InspectionScanStatus::Exhausted)
    );
    let report = store
        .inspect(InspectionLimits {
            time_budget: Duration::from_nanos(1),
            ..Default::default()
        })
        .unwrap();
    assert!(report.limits_hit.contains(&InspectionLimit::Time));
    assert_eq!(report.content_hashes_checked, 0);
    for name in [".pending-first", ".pending-second"] {
        fs::write(store.root().join("artifacts").join(name), b"pending").unwrap();
    }
    let report = store
        .inspect(InspectionLimits {
            max_findings: 1,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(report.findings.len(), 1);
    assert!(report.limits_hit.contains(&InspectionLimit::Findings));
    assert!(
        report
            .scans
            .iter()
            .any(|scan| scan.status == InspectionScanStatus::Limited)
    );
}

#[test]
fn inspection_cannot_recreate_a_missing_database() {
    let (_root, store, _) = fixture();
    fs::remove_file(store.root().join("hunin.sqlite")).unwrap();
    assert!(store.inspect(InspectionLimits::default()).is_err());
    assert!(!store.root().join("hunin.sqlite").exists());
}

#[test]
fn inspection_cannot_migrate_a_newer_schema() {
    let (_root, store, _) = fixture();
    {
        let connection = Connection::open(store.root().join("hunin.sqlite")).unwrap();
        connection.execute_batch("PRAGMA user_version=99").unwrap();
    }
    let before = fs::read(store.root().join("hunin.sqlite")).unwrap();
    let error = store.inspect(InspectionLimits::default()).unwrap_err();
    assert!(error.to_string().contains("newer schema 99"));
    assert_eq!(fs::read(store.root().join("hunin.sqlite")).unwrap(), before);
    let connection = Connection::open(store.root().join("hunin.sqlite")).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        99
    );
}

#[test]
fn workspace_metadata_presence_is_distinct_from_owner_liveness() {
    let (_root, store, _) = fixture();
    store
        .save_document(
            "analysis-results",
            "attempt",
            &json!({"run_id":"recording","status":"running"}),
        )
        .unwrap();
    store
        .save_document("analyses", "package", &json!({}))
        .unwrap();
    let known = [
        "captures/recording",
        "analysis-executions/attempt",
        "analysis-packages/package",
    ];
    for path in known {
        fs::create_dir_all(store.root().join(path)).unwrap();
    }
    let report = store.inspect(InspectionLimits::default()).unwrap();
    for path in known {
        assert!(report.findings.iter().any(|finding| matches!(finding,
            InspectionFinding::WorkspaceOwnershipUnknown { relative_path, metadata_reference: WorkspaceMetadataReference::Present, .. }
            if relative_path == Path::new(path))));
    }
}

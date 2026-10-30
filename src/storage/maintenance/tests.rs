use super::*;
use crate::publication::{PublicationMetadata, PublicationRequest};
use serde_json::json;

fn fixture() -> (tempfile::TempDir, Store, crate::model::runs::Artifact) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store
        .save_run(&Run {
            id: "recording".into(),
            name: "removal control".into(),
            status: RunStatus::Completed,
            command: vec![],
            cwd: ".".into(),
            runtime: "imported".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: None,
            diagnostics: vec![],
            source: Value::Null,
            correctness: Value::Null,
        })
        .unwrap();
    let artifact = store
        .add_artifact_bytes("recording", "raw", "raw", b"owned raw")
        .unwrap();
    std::fs::create_dir_all(root.path().join("captures/recording")).unwrap();
    std::fs::write(
        root.path().join("captures/recording/raw"),
        b"owned workspace",
    )
    .unwrap();
    (root, store, artifact)
}

#[test]
fn interruption_after_metadata_commit_retains_an_exact_retry_receipt() {
    let (root, store, artifact) = fixture();
    store.authorize_removal("recording").unwrap();
    assert!(store.run("recording").is_err());
    assert!(
        root.path()
            .join("artifacts")
            .join(&artifact.sha256)
            .is_file()
    );
    let receipt = store.removal_receipt("recording").unwrap().unwrap();
    assert_eq!(receipt["completed"], false);
    assert_eq!(receipt["artifacts"][0]["sha256"], artifact.sha256);
    drop(store);
    let reopened = Store::open(root.path()).unwrap();
    reopened.remove_run("recording").unwrap();
    assert!(
        !root
            .path()
            .join("artifacts")
            .join(&artifact.sha256)
            .exists()
    );
    assert!(!root.path().join("captures/recording").exists());
    assert_eq!(
        reopened.removal_receipt("recording").unwrap().unwrap()["completed"],
        true
    );
    reopened.remove_run("recording").unwrap();
    assert!(reopened.remove_run("never-authorized").is_err());
}

#[test]
fn retry_preserves_new_references_and_rejects_reused_run_identity() {
    let (root, store, artifact) = fixture();
    let original = store.run("recording").unwrap();
    store.authorize_removal("recording").unwrap();
    let mut other = original.clone();
    other.id = "other".into();
    store.save_run(&other).unwrap();
    store
        .add_artifact_bytes("other", "shared", "raw", b"owned raw")
        .unwrap();
    store.remove_run("recording").unwrap();
    assert_eq!(
        std::fs::read(root.path().join("artifacts").join(&artifact.sha256)).unwrap(),
        b"owned raw"
    );
    assert_eq!(
        store.removal_receipt("recording").unwrap().unwrap()["artifacts"][0]["disposition"],
        "referenced"
    );
    store.save_run(&original).unwrap();
    std::fs::create_dir_all(root.path().join("captures/recording")).unwrap();
    std::fs::write(root.path().join("captures/recording/new"), b"new owner").unwrap();
    assert!(
        store
            .remove_run("recording")
            .unwrap_err()
            .to_string()
            .contains("reused")
    );
    assert!(store.run("recording").is_ok());
    assert_eq!(
        std::fs::read(root.path().join("captures/recording/new")).unwrap(),
        b"new owner"
    );
}

#[test]
fn changed_workspace_is_preserved_and_partial_cleanup_is_inspectable() {
    let (root, store, _artifact) = fixture();
    store.authorize_removal("recording").unwrap();
    std::fs::rename(
        root.path().join("captures/recording"),
        root.path().join("captures/original-retained"),
    )
    .unwrap();
    std::fs::create_dir(root.path().join("captures/recording")).unwrap();
    std::fs::write(root.path().join("captures/recording/new"), b"changed path").unwrap();
    assert!(store.remove_run("recording").is_err());
    let receipt = store.removal_receipt("recording").unwrap().unwrap();
    assert_eq!(receipt["completed"], false);
    assert!(
        receipt["last_error"]
            .as_str()
            .unwrap()
            .contains("identity changed")
    );
    assert_eq!(receipt["artifacts"][0]["disposition"], "removed");
    assert_eq!(
        std::fs::read(root.path().join("captures/recording/new")).unwrap(),
        b"changed path"
    );
    // Return the original owned object to allow only its previously authorized cleanup.
    std::fs::rename(
        root.path().join("captures/recording"),
        root.path().join("captures/unrelated-preserved"),
    )
    .unwrap();
    std::fs::rename(
        root.path().join("captures/original-retained"),
        root.path().join("captures/recording"),
    )
    .unwrap();
    store.remove_run("recording").unwrap();
    assert!(!root.path().join("captures/recording").exists());
    assert!(
        root.path()
            .join("captures/unrelated-preserved/new")
            .is_file()
    );
}

#[test]
fn missing_after_an_uncommitted_cleanup_step_is_idempotent() {
    let (root, store, artifact) = fixture();
    store.authorize_removal("recording").unwrap();
    // Models interruption after unlink but before progress commits.
    std::fs::remove_file(root.path().join("artifacts").join(artifact.sha256)).unwrap();
    std::fs::remove_dir_all(root.path().join("captures/recording")).unwrap();
    store.remove_run("recording").unwrap();
    let receipt = store.removal_receipt("recording").unwrap().unwrap();
    assert_eq!(receipt["completed"], true);
    assert_eq!(receipt["artifacts"][0]["disposition"], "missing");
    assert_eq!(receipt["paths"][0]["disposition"], "missing");
}

#[test]
fn corrupt_digest_cannot_escape_the_artifact_directory_or_delete_metadata() {
    let (root, store, artifact) = fixture();
    let outside = root.path().join("outside");
    std::fs::write(&outside, b"preserve").unwrap();
    let db = store.connection().unwrap();
    db.execute(
        "UPDATE artifacts SET sha256='../outside' WHERE id=?1",
        [artifact.id],
    )
    .unwrap();
    assert!(store.remove_run("recording").is_err());
    assert!(store.run("recording").is_ok());
    assert!(store.removal_receipt("recording").unwrap().is_none());
    assert_eq!(std::fs::read(outside).unwrap(), b"preserve");
}

#[test]
fn an_execution_id_reused_by_another_run_preserves_its_workspace() {
    let (root, store, _) = fixture();
    store
        .save_document(
            "analysis-results",
            "attempt",
            &json!({"run_id":"recording","status":"accepted"}),
        )
        .unwrap();
    std::fs::create_dir_all(root.path().join("analysis-executions/attempt")).unwrap();
    std::fs::write(
        root.path().join("analysis-executions/attempt/input.json"),
        b"old",
    )
    .unwrap();
    let mut other = store.run("recording").unwrap();
    other.id = "other".into();
    store.save_run(&other).unwrap();
    store.authorize_removal("recording").unwrap();
    store
        .save_document(
            "analysis-results",
            "attempt",
            &json!({"run_id":"other","status":"running"}),
        )
        .unwrap();
    assert!(store.remove_run("recording").is_err());
    assert!(
        root.path()
            .join("analysis-executions/attempt/input.json")
            .is_file()
    );
}

#[cfg(unix)]
#[test]
fn linked_parents_are_not_authorized_and_linked_children_do_not_follow_targets() {
    use std::os::unix::fs::symlink;
    let (root, store, _) = fixture();
    let external = tempfile::tempdir().unwrap();
    std::fs::write(external.path().join("preserve"), b"outside").unwrap();
    std::fs::remove_dir_all(root.path().join("captures/recording")).unwrap();
    symlink(external.path(), root.path().join("captures/recording")).unwrap();
    store.remove_run("recording").unwrap();
    assert_eq!(
        std::fs::read(external.path().join("preserve")).unwrap(),
        b"outside"
    );
    assert!(!root.path().join("captures/recording").exists());

    let (root, store, _) = fixture();
    std::fs::remove_dir_all(root.path().join("captures")).unwrap();
    symlink(external.path(), root.path().join("captures")).unwrap();
    assert!(store.remove_run("recording").is_err());
    assert!(store.run("recording").is_ok());
    assert_eq!(
        std::fs::read(external.path().join("preserve")).unwrap(),
        b"outside"
    );
}

#[test]
fn concurrent_removal_and_publication_cannot_recreate_accepted_rows() {
    let (_root, store, _) = fixture();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let publishing = store.clone();
    let ready = barrier.clone();
    let publication = std::thread::spawn(move || {
        ready.wait();
        publishing.accept("recording","raced",&[json!({"schema_version":1,"kind":"event","id":"raced","run_id":"recording","producer_id":"control","pid":1,"tid":"","timestamp_us":1,"name":"raced","basis":"observed"})])
    });
    barrier.wait();
    store.remove_run("recording").unwrap();
    let _serialized_outcome = publication.join().unwrap();
    assert!(store.run("recording").is_err());
    let connection = store.connection().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE run_id='recording'",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM result_sets WHERE run_id='recording'",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        store.removal_receipt("recording").unwrap().unwrap()["completed"],
        true
    );
}

#[test]
fn concurrent_reselection_and_derived_publication_never_select_stale_inputs() {
    let (_root, store, _) = fixture();
    let event = |id: &str| json!({"schema_version":1,"kind":"event","id":id,"run_id":"recording","producer_id":"control","pid":1,"tid":"","timestamp_us":1,"name":id,"basis":"observed"});
    store
        .accept_import("recording", "raw", "v1", &[event("raw-v1")])
        .unwrap();
    store
        .accept("recording", "independent", &[event("independent")])
        .unwrap();
    let input = store
        .selected_result_id("recording", "raw")
        .unwrap()
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let publishing = store.clone();
    let ready = barrier.clone();
    let publication = std::thread::spawn(move || {
        ready.wait();
        publishing.publish_result(PublicationRequest {
            run_id: "recording",
            source_key: "derived",
            input_identity: "derived-v1",
            records: &[json!({"schema_version":1,"kind":"event","id":"derived","run_id":"recording","producer_id":"analysis","pid":1,"tid":"","timestamp_us":2,"name":"derived","basis":"derived","evidence":["raw-v1"]})],
            metadata: Some(PublicationMetadata::Analysis {
                execution_id: "attempt",
                document: &json!({"run_id":"recording","status":"accepted","input_result_sets":[input]}),
            }),
            deadline: None,
        })
    });
    barrier.wait();
    store
        .accept_import("recording", "raw", "v2", &[event("raw-v2")])
        .unwrap();
    let _serialized_outcome = publication.join().unwrap();
    let records = store.records("recording").unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().any(|row| row["id"] == "raw-v2"));
    assert!(records.iter().any(|row| row["id"] == "independent"));
    assert!(
        store
            .selected_result_id("recording", "derived")
            .unwrap()
            .is_none()
    );
}

#[test]
fn unsupported_or_extended_receipts_remain_inspectable_and_are_never_rewritten() {
    for future in [false, true] {
        let (root, store, artifact) = fixture();
        store.authorize_removal("recording").unwrap();
        let mut receipt = store.removal_receipt("recording").unwrap().unwrap();
        if future {
            receipt["version"] = json!(2);
        } else {
            receipt["unknown_extension"] = json!({"preserve":true});
        }
        store
            .save_document(NAMESPACE, "recording", &receipt)
            .unwrap();
        assert!(store.remove_run("recording").is_err());
        assert_eq!(
            store.removal_receipt("recording").unwrap().unwrap(),
            receipt
        );
        assert!(
            root.path()
                .join("artifacts")
                .join(artifact.sha256)
                .is_file()
        );
        assert!(root.path().join("captures/recording/raw").is_file());
    }
}

#[test]
fn oversized_receipt_payload_is_rejected_without_cleanup_or_rewrite() {
    let (root, store, artifact) = fixture();
    store.authorize_removal("recording").unwrap();
    let oversized = "x".repeat(RECEIPT_BYTE_LIMIT + 1);
    let db = store.connection().unwrap();
    db.execute(
        "UPDATE documents SET payload=?1 WHERE namespace=?2 AND id='recording'",
        params![oversized, NAMESPACE],
    )
    .unwrap();
    assert!(
        store
            .remove_run("recording")
            .unwrap_err()
            .to_string()
            .contains("byte bound")
    );
    assert!(store.removal_receipt("recording").is_err());
    assert_eq!(
        db.query_row(
            "SELECT length(CAST(payload AS BLOB)) FROM documents WHERE namespace=?1",
            [NAMESPACE],
            |r| r.get::<_, usize>(0)
        )
        .unwrap(),
        RECEIPT_BYTE_LIMIT + 1
    );
    assert!(
        root.path()
            .join("artifacts")
            .join(artifact.sha256)
            .is_file()
    );
    assert!(root.path().join("captures/recording/raw").is_file());
}

#[test]
fn corrupt_scalar_receipt_has_no_object_contract_or_deletion_authority() {
    let (root, store, artifact) = fixture();
    store.authorize_removal("recording").unwrap();
    store
        .save_document(NAMESPACE, "recording", &json!("invalid scalar"))
        .unwrap();
    assert!(
        store
            .removal_receipt("recording")
            .unwrap_err()
            .to_string()
            .contains("not an object")
    );
    assert!(store.remove_run("recording").is_err());
    assert!(
        root.path()
            .join("artifacts")
            .join(artifact.sha256)
            .is_file()
    );
    assert!(root.path().join("captures/recording/raw").is_file());
    assert_eq!(
        store.document(NAMESPACE, "recording").unwrap(),
        json!("invalid scalar")
    );
}

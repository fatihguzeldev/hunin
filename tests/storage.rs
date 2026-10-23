use hunin::{model::runs::Run, storage::Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

fn run() -> Run {
    Run {
        id: "run-1".into(),
        name: "control".into(),
        status: "completed".into(),
        command: vec!["control".into()],
        cwd: "/tmp".into(),
        runtime: "native".into(),
        started_at_us: 1,
        finished_at_us: Some(5),
        exit_code: Some(0),
        diagnostics: vec![],
        source: json!({"fingerprint":"source-1"}),
        correctness: json!({"passed":true}),
    }
}
fn event(id: &str) -> Value {
    json!({"schema_version":1,"kind":"event","id":id,"run_id":"run-1","producer_id":"process-lifetime-1","pid":123,"tid":"main","timestamp_us":2,"name":"control","basis":"observed"})
}

#[test]
fn accepted_inputs_are_atomic_idempotent_and_survive_reopening() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let a = event("a");
    let mut bad = event("bad");
    bad["parent_id"] = json!("missing");
    assert!(
        store
            .accept("run-1", "input-bad", &[a.clone(), bad])
            .is_err()
    );
    assert!(store.records("run-1").unwrap().is_empty());
    assert!(store.accept("run-1", "input-a", &[a.clone()]).unwrap());
    assert!(!store.accept("run-1", "input-a", &[a]).unwrap());
    drop(store);
    let reopened = Store::open(root.path()).unwrap();
    assert_eq!(reopened.run("run-1").unwrap().name, "control");
    assert_eq!(reopened.records("run-1").unwrap().len(), 1);
    let mut cross_run = event("other");
    cross_run["run_id"] = json!("run-2");
    assert!(reopened.accept("run-1", "wrong-run", &[cross_run]).is_err());
}

#[test]
fn shared_raw_artifacts_survive_removing_one_run() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let first = run();
    store.save_run(&first).unwrap();
    let mut second = run();
    second.id = "run-2".into();
    store.save_run(&second).unwrap();
    let source = root.path().join("raw.json");
    std::fs::write(&source, b"immutable input").unwrap();
    let a = store.add_artifact("run-1", &source, "control").unwrap();
    let b = store.add_artifact("run-2", &source, "control").unwrap();
    assert_eq!(a.sha256, b.sha256);
    store.remove_run("run-1").unwrap();
    assert_eq!(
        std::fs::read(store.artifact_path("run-2", &b.id).unwrap()).unwrap(),
        b"immutable input"
    );
}

#[test]
fn large_artifacts_stream_to_immutable_shared_bytes_and_preserve_reference_identity() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let source = root.path().join("capture.etl");
    let chunk: Vec<u8> = (0..65536).map(|n| (n % 251) as u8).collect();
    let mut output = std::fs::File::create(&source).unwrap();
    let mut expected = Sha256::new();
    for _ in 0..256 {
        output.write_all(&chunk).unwrap();
        expected.update(&chunk);
    }
    drop(output);
    let artifact = store.add_artifact("run-1", &source, "etl").unwrap();
    assert_eq!(artifact.size, 16 * 1024 * 1024);
    assert_eq!(artifact.sha256, format!("{:x}", expected.finalize()));
    let path = store.artifact_path("run-1", &artifact.id).unwrap();
    let mut retained = std::fs::File::open(&path).unwrap();
    let mut actual = vec![0u8; chunk.len()];
    for _ in 0..256 {
        retained.read_exact(&mut actual).unwrap();
        assert_eq!(actual, chunk);
    }
    assert_eq!(retained.read(&mut actual).unwrap(), 0);

    let small = root.path().join("control.raw");
    std::fs::write(&small, &chunk).unwrap();
    let file_reference = store.add_artifact("run-1", &small, "raw").unwrap();
    let byte_reference = store
        .add_artifact_bytes("run-1", "control.raw", "raw", &chunk)
        .unwrap();
    assert_eq!(file_reference.id, byte_reference.id);
    assert_eq!(file_reference.sha256, byte_reference.sha256);
    assert_eq!(store.artifacts("run-1").unwrap().len(), 2);
    assert_eq!(
        std::fs::read_dir(root.path().join("artifacts"))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn artifact_failures_remove_staging_files_and_preserve_existing_bytes() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let source = root.path().join("source.ts");
    std::fs::File::create(&source)
        .unwrap()
        .set_len(2 * 1024 * 1024 + 1)
        .unwrap();
    assert!(
        store
            .add_artifact("run-1", &source, "resolved-source")
            .is_err()
    );
    assert!(
        store
            .add_artifact_bytes(
                "run-1",
                "source.ts",
                "resolved-source",
                &vec![0; 2 * 1024 * 1024 + 1]
            )
            .is_err()
    );
    assert!(store.add_artifact("run-1", root.path(), "raw").is_err());
    assert_eq!(
        std::fs::read_dir(root.path().join("artifacts"))
            .unwrap()
            .count(),
        0
    );
    assert!(store.artifacts("run-1").unwrap().is_empty());

    std::fs::write(&source, b"immutable").unwrap();
    let artifact = store.add_artifact("run-1", &source, "raw").unwrap();
    let path = store.artifact_path("run-1", &artifact.id).unwrap();
    std::fs::write(&path, b"corrupted").unwrap();
    let mut second = run();
    second.id = "run-2".into();
    store.save_run(&second).unwrap();
    assert!(
        store
            .add_artifact("run-2", &source, "raw")
            .unwrap_err()
            .to_string()
            .contains("identity mismatch")
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"corrupted");
    assert!(store.artifacts("run-2").unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(root.path().join("artifacts"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn artifact_publication_and_run_collection_share_the_same_lock() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let mut second = run();
    second.id = "run-2".into();
    store.save_run(&second).unwrap();
    let source = root.path().join("capture.raw");
    std::fs::write(&source, vec![42; 256 * 1024]).unwrap();
    store.add_artifact("run-1", &source, "raw").unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let other = store.clone();
    let ready = barrier.clone();
    let remove = std::thread::spawn(move || {
        ready.wait();
        other.remove_run("run-1").unwrap();
    });
    barrier.wait();
    let artifact = store.add_artifact("run-2", &source, "raw").unwrap();
    remove.join().unwrap();
    assert_eq!(
        std::fs::read(store.artifact_path("run-2", &artifact.id).unwrap()).unwrap(),
        vec![42; 256 * 1024]
    );
    assert_eq!(
        std::fs::read_dir(root.path().join("artifacts"))
            .unwrap()
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn removing_the_owner_during_a_stream_cleans_unpublished_bytes() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let source = root.path().join("stream.raw");
    let path = std::ffi::CString::new(source.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let other = store.clone();
    let import = std::thread::spawn(move || other.add_artifact("run-1", &source, "raw"));
    // Opening the writer proves the importer passed its initial ownership
    // check and opened the input. Keep EOF blocked until its owner is removed.
    let mut writer = std::fs::File::create(root.path().join("stream.raw")).unwrap();
    writer.write_all(b"partial capture").unwrap();
    store.remove_run("run-1").unwrap();
    drop(writer);
    assert!(
        import
            .join()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("removed before artifact publication")
    );
    assert_eq!(
        std::fs::read_dir(root.path().join("artifacts"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn opening_a_newer_database_does_not_rewrite_its_schema_version() {
    let root = tempfile::tempdir().unwrap();
    Store::open(root.path()).unwrap();
    let connection = rusqlite::Connection::open(root.path().join("hunin.sqlite")).unwrap();
    connection.execute_batch("PRAGMA user_version=999").unwrap();
    assert!(
        Store::open(root.path())
            .err()
            .unwrap()
            .to_string()
            .contains("newer Hunin")
    );
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        999
    );
}

#[test]
fn distinct_artifact_names_share_bytes_without_losing_their_roles() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let stdout = root.path().join("stdout.txt");
    let stderr = root.path().join("stderr.txt");
    std::fs::write(&stdout, b"").unwrap();
    std::fs::write(&stderr, b"").unwrap();
    let a = store.add_artifact("run-1", &stdout, "stdout").unwrap();
    let b = store.add_artifact("run-1", &stderr, "stderr").unwrap();
    assert_eq!(a.sha256, b.sha256);
    assert_ne!(a.id, b.id);
    assert_eq!(store.artifacts("run-1").unwrap().len(), 2);
}

#[test]
fn references_preserve_kind_and_acyclic_parent_context_ownership() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let mut a = event("a");
    a["context_id"] = json!("b");
    let b = event("b");
    assert!(store.accept("run-1", "bad-context", &[a, b]).is_err());
    let mut a = event("a");
    a["kind"] = json!("span");
    a["duration_us"] = json!(1);
    a["parent_id"] = json!("b");
    let mut b = a.clone();
    b["id"] = json!("b");
    b["parent_id"] = json!("a");
    assert!(store.accept("run-1", "parent-cycle", &[a, b]).is_err());
}

#[test]
fn reanalysis_selects_one_normalization_and_preserves_history() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    store
        .accept_import(
            "run-1",
            "raw-recording",
            "importer-v1-sidecar-a",
            &[event("old")],
        )
        .unwrap();
    store
        .accept_import(
            "run-1",
            "raw-recording",
            "importer-v1-sidecar-b",
            &[event("new")],
        )
        .unwrap();
    assert_eq!(store.records("run-1").unwrap().len(), 1);
    assert_eq!(store.records("run-1").unwrap()[0]["id"], "new");
    assert_eq!(store.records_history("run-1").unwrap().len(), 2);
    store
        .accept_import(
            "run-1",
            "raw-recording",
            "importer-v1-sidecar-a",
            &[event("old")],
        )
        .unwrap();
    assert_eq!(store.records("run-1").unwrap()[0]["id"], "old");
}

#[test]
fn reanalysis_invalidates_dependents_and_keeps_independent_results() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    store.accept("run-1", "raw", &[event("raw")]).unwrap();
    let raw = store.selected_result_id("run-1", "raw").unwrap().unwrap();
    let publish = |source: &str, id: &str, inputs: Vec<String>| {
        let mut record = event(id);
        record["basis"] = json!("derived");
        let doc = json!({"run_id":"run-1","status":"accepted","input_result_sets":inputs});
        store.publish(
            "run-1",
            source,
            id,
            &[record],
            Some(("analysis-results", id, &doc)),
        )
    };
    publish("analysis:b", "b1", vec![raw.clone()]).unwrap();
    publish("analysis:a", "a1", vec!["b1".into()]).unwrap();
    assert_eq!(
        store.active_result_ids("run-1", "analysis:b").unwrap(),
        vec![raw.clone()]
    );
    publish("analysis:a", "a2", vec!["b1".into()]).unwrap();
    assert_eq!(
        store
            .selected_result_id("run-1", "analysis:b")
            .unwrap()
            .as_deref(),
        Some("b1")
    );
    publish("analysis:b", "b2", vec![raw]).unwrap();
    assert!(
        store
            .selected_result_id("run-1", "analysis:a")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .records_for_result_ids("run-1", &["b2".into()])
            .unwrap()[0]["id"],
        "b2"
    );
    assert!(publish("analysis:a", "a2", vec!["b1".into()]).is_err());
    assert_eq!(store.records("run-1").unwrap().len(), 2);
    assert_eq!(store.records_history("run-1").unwrap().len(), 5);
}

#[test]
fn persisted_cancellation_wins_the_publication_race_and_owned_documents_are_removed() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let doc = json!({"run_id":"run-1","status":"running"});
    store
        .save_document("analysis-results", "attempt", &doc)
        .unwrap();
    let other_client = Store::open(root.path()).unwrap();
    assert_eq!(
        other_client.request_analysis_cancel("attempt").unwrap()["cancellation_requested"],
        true
    );
    assert!(
        store
            .publish(
                "run-1",
                "analysis:a",
                "a1",
                &[event("a")],
                Some((
                    "analysis-results",
                    "attempt",
                    &json!({"run_id":"run-1","status":"accepted"})
                ))
            )
            .is_err()
    );
    assert!(store.records("run-1").unwrap().is_empty());
    assert!(store.remove_run("run-1").is_err());
    store
        .save_document(
            "analysis-results",
            "attempt",
            &json!({"run_id":"run-1","status":"cancelled"}),
        )
        .unwrap();
    assert_eq!(
        other_client.request_analysis_cancel("attempt").unwrap()["status"],
        "cancelled"
    );
    store
        .save_document("cancellations", "run-1", &json!({"run_id":"run-1"}))
        .unwrap();
    let capture = root.path().join("captures/run-1");
    let attempt = root.path().join("analysis-executions/attempt");
    std::fs::create_dir_all(&capture).unwrap();
    std::fs::create_dir_all(&attempt).unwrap();
    std::fs::write(capture.join("raw.txt"), "raw").unwrap();
    std::fs::write(attempt.join("input.json"), "{}").unwrap();
    store.remove_run("run-1").unwrap();
    assert!(store.document("analysis-results", "attempt").is_err());
    assert!(store.document("cancellations", "run-1").is_err());
    assert!(!capture.exists());
    assert!(!attempt.exists());
}

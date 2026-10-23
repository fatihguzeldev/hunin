use hunin::{model::runs::Run, operations::Operations, storage::Store};
use serde_json::json;

fn run() -> Run {
    Run {
        id: "import-owner".into(),
        name: "measured recording".into(),
        status: "completed".into(),
        command: vec!["workload".into()],
        cwd: "/recorded/project".into(),
        runtime: "none".into(),
        started_at_us: 1,
        finished_at_us: Some(5),
        exit_code: Some(0),
        diagnostics: vec!["final collection diagnostic".into()],
        source: json!({"fingerprint":"final-source"}),
        correctness: json!({"passed":true}),
    }
}

#[tokio::test]
async fn active_recording_rejects_import_before_preserving_or_accepting_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let mut running = run();
    running.status = "running".into();
    running.finished_at_us = None;
    store.save_run(&running).unwrap();
    let path = directory.path().join("empty.jsonl");
    std::fs::write(&path, "").unwrap();
    let operations = Operations::new(store.clone());
    let result = operations
        .call(
            "recordings.import",
            json!({"run_id":running.id,"path":path,"format":"hunin-jsonl"}),
        )
        .await;
    assert!(
        result.is_err(),
        "active recording accepted an import: {result:?}"
    );
    assert!(
        result.unwrap_err().to_string().contains("running"),
        "import must explain that the recording is active"
    );
    assert!(store.artifacts(&running.id).unwrap().is_empty());
    assert!(store.records(&running.id).unwrap().is_empty());
    assert_eq!(store.run(&running.id).unwrap().status, "running");
}

#[test]
fn import_completion_preserves_latest_execution_manifest_and_accepted_records() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let original = run();
    store.save_run(&original).unwrap();
    // The importer read this manifest before normalization. Another writer
    // finalizes the recording before the importer adds its diagnostics.
    let stale = store.run(&original.id).unwrap();
    let mut finalized = original.clone();
    finalized.status = "cancelled".into();
    finalized.finished_at_us = Some(20);
    finalized.exit_code = Some(130);
    finalized.correctness = json!({"passed":false,"reason":"cancelled"});
    finalized.source = json!({"fingerprint":"finalized-source","pid":123});
    finalized
        .diagnostics
        .push("finalized after import started".into());
    store.save_run(&finalized).unwrap();
    let event = json!({"schema_version":1,"kind":"event","id":"imported-event","run_id":stale.id,"producer_id":"import","pid":123,"tid":"main","timestamp_us":2,"name":"control","basis":"observed"});
    store
        .accept(&stale.id, "accepted-import", &[event.clone()])
        .unwrap();
    let merged = store
        .finish_recording_import(&stale.id, &["import diagnostic".into()], false)
        .unwrap();
    finalized.diagnostics.push("import diagnostic".into());
    assert_eq!(
        serde_json::to_value(&merged).unwrap(),
        serde_json::to_value(&finalized).unwrap()
    );
    assert_eq!(store.records(&stale.id).unwrap(), vec![event]);
}

#[test]
fn import_completion_cannot_recreate_a_removed_run_or_overwrite_an_active_one() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let original = run();
    store.save_run(&original).unwrap();
    let stale = store.run(&original.id).unwrap();
    store.remove_run(&stale.id).unwrap();
    for complete in [false, true] {
        assert!(
            store
                .finish_recording_import(&stale.id, &["late import".into()], complete)
                .is_err()
        );
        assert!(store.run(&stale.id).is_err());
    }
    let mut active = original;
    active.status = "running".into();
    active.finished_at_us = None;
    store.save_run(&active).unwrap();
    assert!(
        store
            .finish_recording_import(&stale.id, &["late import".into()], false)
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(store.run(&active.id).unwrap()).unwrap(),
        serde_json::to_value(active).unwrap()
    );
}

#[tokio::test]
async fn new_import_completes_and_failed_import_retains_its_raw_input() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let operations = Operations::new(store.clone());
    let path = directory.path().join("empty.jsonl");
    std::fs::write(&path, "").unwrap();
    let imported = operations
        .call(
            "recordings.import",
            json!({"path":path,"format":"hunin-jsonl"}),
        )
        .await
        .unwrap();
    assert_eq!(imported["run"]["status"], "completed");
    std::fs::write(&path, "invalid json\n").unwrap();
    assert!(
        operations
            .call(
                "recordings.import",
                json!({"path":path,"format":"hunin-jsonl"})
            )
            .await
            .is_err()
    );
    let failed = store
        .runs(10, 0)
        .unwrap()
        .into_iter()
        .find(|run| run.status == "failed")
        .unwrap();
    assert!(
        failed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("Import failed"))
    );
    let artifacts = store.artifacts(&failed.id).unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(
        std::fs::read(store.artifact_path(&failed.id, &artifacts[0].id).unwrap()).unwrap(),
        b"invalid json\n"
    );
}

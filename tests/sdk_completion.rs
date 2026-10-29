use hunin::{execution::Engine, model::runs::Run, storage::Store};
use serde_json::json;

async fn record(mode: &str) -> (tempfile::TempDir, Store, Run) {
    let project = tempfile::tempdir().unwrap();
    let store = Store::open(&project.path().join("data")).unwrap();
    let engine = Engine::new(store.clone());
    let workload =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/javascript/test/completion.mjs");
    let command = vec![
        "node".to_owned(),
        workload.display().to_string(),
        mode.into(),
    ];
    let run = engine
        .start(
            serde_json::from_value(json!({
                "command":command,"cwd":project.path(),"runtime":"none","timeout_seconds":30,
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let run = engine.wait(&run.id).await.unwrap();
    (project, store, run)
}

#[tokio::test]
async fn sdk_abrupt_exit_distinguishes_retained_rows_from_unknown_delivery() {
    for (mode, status, count) in [("buffered", "empty", 0), ("flushed", "accepted", 1)] {
        let (_project, store, run) = record(mode).await;
        assert_eq!(run.status, "failed", "{mode}: {:?}", run.diagnostics);
        assert_eq!(run.source["target_outcome"]["status"], "failed");
        let ingestion = &run.source["sdk_ingestion"];
        assert_eq!(ingestion["status"], status, "{mode}: {:?}", run.diagnostics);
        assert_eq!(ingestion["evidence"]["completion"], "unknown");
        assert_eq!(ingestion["evidence"]["loss"], "unknown");
        assert_eq!(
            ingestion["evidence"]["accepted_producer_declarations"]["count"],
            count
        );
        assert_eq!(
            store
                .records(&run.id)
                .unwrap()
                .iter()
                .filter(|row| row["kind"] == "event")
                .count(),
            count
        );
    }
}

#[tokio::test]
async fn sdk_unterminated_valid_row_remains_accepted_but_truncated_json_rejects_the_whole_spool() {
    for (mode, expected_status, count) in [("truncated", "rejected", 0), ("legacy", "accepted", 1)]
    {
        let (_project, store, run) = record(mode).await;
        assert_eq!(
            run.source["target_outcome"]["status"], "completed",
            "{:?}",
            run.diagnostics
        );
        let ingestion = &run.source["sdk_ingestion"];
        assert_eq!(ingestion["status"], expected_status);
        assert_eq!(ingestion["evidence"]["completion"], "unknown");
        assert_eq!(ingestion["evidence"]["loss"], "unknown");
        assert_eq!(
            ingestion["evidence"]["spool_snapshot"]["retained_artifact_count"],
            if mode == "truncated" { 2 } else { 1 }
        );
        assert_eq!(
            ingestion["evidence"]["spool_snapshot"]["nonempty_without_final_lf_count"],
            1
        );
        assert_eq!(
            ingestion["evidence"]["spool_snapshot"]["inventory_retained"],
            true
        );
        assert_eq!(
            store
                .records(&run.id)
                .unwrap()
                .iter()
                .filter(|row| row["kind"] == "event")
                .count(),
            count
        );
        if mode == "truncated" {
            assert_eq!(run.status, "failed");
            assert_eq!(ingestion["stage"], "decode");
            let artifact = store
                .artifacts(&run.id)
                .unwrap()
                .into_iter()
                .find(|artifact| {
                    artifact.name.starts_with("sdk-js-") && artifact.name.ends_with(".jsonl")
                })
                .unwrap();
            let bytes = std::fs::read(store.artifact_path(&run.id, &artifact.id).unwrap()).unwrap();
            assert!(bytes.ends_with(b"{\"schema_version\":1"));
            assert!(
                store
                    .artifacts(&run.id)
                    .unwrap()
                    .iter()
                    .any(|artifact| artifact.name == "z-later.jsonl")
            );
        } else {
            assert_eq!(run.status, "completed", "{:?}", run.diagnostics);
        }
    }
}

#[tokio::test]
async fn sdk_accepted_producer_declarations_are_bounded_and_self_reported() {
    let (_project, store, run) = record("declarations").await;
    assert_eq!(run.status, "completed", "{:?}", run.diagnostics);
    let declarations = &run.source["sdk_ingestion"]["evidence"]["accepted_producer_declarations"];
    assert_eq!(declarations["count"], 73);
    assert_eq!(declarations["truncated"], true);
    assert_eq!(declarations["source"], "accepted_record_fields");
    assert_eq!(declarations["entry_limit"], 64);
    assert_eq!(declarations["producer_id_byte_limit"], 256);
    let entries = declarations["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 64);
    assert!(
        entries
            .iter()
            .all(|entry| entry["pid_json_number"].is_string()
                && entry["producer_id"].as_str().unwrap().len() <= 256)
    );
    let records = store.records(&run.id).unwrap();
    for id in ["large-pid", "non-u64-pid"] {
        let record = records.iter().find(|record| record["id"] == id).unwrap();
        let entry = entries
            .iter()
            .find(|entry| entry["producer_id"] == record["producer_id"])
            .unwrap();
        assert_eq!(entry["pid_json_number"], record["pid"].to_string());
        if id == "large-pid" {
            assert_eq!(entry["pid_json_number"], "9007199254740993");
        }
    }
    assert_eq!(
        store
            .records(&run.id)
            .unwrap()
            .iter()
            .filter(|row| row["kind"] == "event")
            .count(),
        73
    );
}

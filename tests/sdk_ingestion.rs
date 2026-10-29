use hunin::{execution::Engine, storage::Store};
use serde_json::json;

#[tokio::test]
async fn malformed_and_schema_invalid_sdk_data_preserve_target_success_and_raw_evidence() {
    for (contents, stage) in [
        ("not-json\n", "decode"),
        ("{\"kind\":\"event\"}\n", "acceptance"),
    ] {
        let project = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        let engine = Engine::new(store.clone());
        let run = engine.start(serde_json::from_value(json!({
            "command": ["node", "-e", "require('fs').writeFileSync(require('path').join(process.env.HUNIN_RECORD_DIR,'bad.jsonl'),process.argv[1]); console.log('target-ok')", contents],
            "cwd": project.path(), "runtime": "none", "expected_stdout": "target-ok\n"
        })).unwrap()).await.unwrap();
        let run = engine.wait(&run.id).await.unwrap();
        assert_eq!(run.status, "failed", "{stage}: {:?}", run.diagnostics);
        assert_eq!(run.correctness["passed"], true);
        assert_eq!(run.source["target_outcome"]["status"], "completed");
        assert_eq!(run.source["sdk_ingestion"]["status"], "rejected");
        assert_eq!(run.source["sdk_ingestion"]["stage"], stage);
        let artifact = store
            .artifacts(&run.id)
            .unwrap()
            .into_iter()
            .find(|artifact| artifact.name == "bad.jsonl")
            .unwrap();
        assert_eq!(
            std::fs::read(store.artifact_path(&run.id, &artifact.id).unwrap()).unwrap(),
            contents.as_bytes()
        );
        assert!(
            store
                .records(&run.id)
                .unwrap()
                .iter()
                .all(|record| record["kind"] != "event")
        );
    }
}

#[tokio::test]
async fn actual_sdk_write_failure_preserves_target_behavior_and_rejects_unreadable_spool() {
    let project = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    let engine = Engine::new(store.clone());
    let workload = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("sdk/javascript/test/write-error.mjs");
    let run = engine
        .start(
            serde_json::from_value(json!({
                "command":["node",workload,"--block-owned-directory"],
                "cwd":project.path(),
                "runtime":"none",
                "expected_stdout":"target-ok\n",
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let run = engine.wait(&run.id).await.unwrap();
    assert_eq!(run.status, "failed", "{:?}", run.diagnostics);
    assert_eq!(run.exit_code, Some(0));
    assert_eq!(run.correctness["passed"], true);
    assert_eq!(run.source["target_outcome"]["status"], "completed");
    assert_eq!(run.source["sdk_ingestion"]["status"], "rejected");
    assert_eq!(run.source["sdk_ingestion"]["stage"], "decode");
    assert!(
        store
            .records(&run.id)
            .unwrap()
            .iter()
            .all(|record| record["kind"] != "event")
    );
    let artifact = store
        .artifacts(&run.id)
        .unwrap()
        .into_iter()
        .find(|artifact| artifact.name == "sdk")
        .expect("blocked SDK evidence retained");
    assert_eq!(
        std::fs::read(store.artifact_path(&run.id, &artifact.id).unwrap()).unwrap(),
        b"blocked-sdk-output",
    );
}

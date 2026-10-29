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

#[tokio::test]
async fn sdk_error_formatters_preserve_target_exceptions_and_accept_valid_spools() {
    for runtime in ["node", "python"] {
        let project = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        let engine = Engine::new(store.clone());
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let (command, expected) = if runtime == "node" {
            (
                vec![
                    "node".to_owned(),
                    root.join("sdk/javascript/test/callback-error.mjs")
                        .display()
                        .to_string(),
                ],
                5,
            )
        } else {
            (vec![if cfg!(windows) { "python" } else { "python3" }.to_owned(), "-c".into(),
                "import hunin\nclass TargetError(Exception):\n def __str__(self): raise RuntimeError('formatter-error')\noriginal=TargetError()\ntry:\n with hunin.context('request'), hunin.span('work'): raise original\nexcept BaseException as actual:\n assert actual is original\nhunin.flush()\nassert hunin.last_error() is None\nprint('target-ok')".into()], 2)
        };
        let run = engine
            .start(
                serde_json::from_value(json!({
                    "command":command,"cwd":project.path(),"runtime":"none",
                    "env":{"PYTHONPATH":root.join("sdk/python")},"expected_stdout":"target-ok\n",
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let run = engine.wait(&run.id).await.unwrap();
        assert_eq!(run.status, "completed", "{runtime}: {:?}", run.diagnostics);
        assert_eq!(run.exit_code, Some(0));
        assert_eq!(run.correctness["passed"], true);
        assert_eq!(run.source["sdk_ingestion"]["status"], "accepted");
        assert_eq!(run.source["sdk_ingestion"]["record_count"], expected);
        let records = store.records(&run.id).unwrap();
        for span in records.iter().filter(|record| record["kind"] == "span") {
            assert_eq!(span["attributes"]["error"], "Error message unavailable");
        }
    }
}

#[tokio::test]
async fn python_boolean_counters_leave_a_valid_numeric_sdk_spool() {
    let project = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    let engine = Engine::new(store.clone());
    let python = if cfg!(windows) { "python" } else { "python3" };
    let run = engine.start(serde_json::from_value(json!({
        "command":[python,"-c","import hunin\nfor value in (False, True):\n assert hunin.counter('boolean', value) is None\nfor value in (0, -2, 3, 2.5):\n assert hunin.counter('numeric', value) is not None\nhunin.flush()\nassert hunin.last_error() is None\nprint('target-ok')"],
        "cwd":project.path(),"runtime":"none",
        "env":{"PYTHONPATH":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/python")},
        "expected_stdout":"target-ok\n",
    })).unwrap()).await.unwrap();
    let run = engine.wait(&run.id).await.unwrap();
    assert_eq!(run.status, "completed", "{:?}", run.diagnostics);
    assert_eq!(run.correctness["passed"], true);
    assert_eq!(run.source["sdk_ingestion"]["status"], "accepted");
    assert_eq!(run.source["sdk_ingestion"]["record_count"], 4);
    let values = store
        .records(&run.id)
        .unwrap()
        .into_iter()
        .filter(|record| record["name"] == "numeric")
        .map(|record| record["value"].clone())
        .collect::<Vec<_>>();
    assert_eq!(values, vec![json!(0), json!(-2), json!(3), json!(2.5)]);
}

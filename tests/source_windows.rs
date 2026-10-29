use hunin::{
    model::runs::Run,
    operations::{Operations, catalog},
    storage::Store,
};
use serde_json::{Value, json};
use std::fs;

fn assert_source_contract(value: &Value) {
    let operation = catalog()
        .iter()
        .find(|entry| entry["name"] == "source.read")
        .unwrap();
    jsonschema::validator_for(&operation["outputSchema"])
        .unwrap()
        .validate(value)
        .unwrap();
}

#[tokio::test]
async fn source_continuation_reads_beyond_line_5000_and_retains_measured_bytes() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    fs::create_dir(&project).unwrap();
    let source = project.join("large.ts");
    let text = (1..=6003)
        .map(|line| format!("line-{line}\n"))
        .collect::<String>();
    fs::write(&source, &text).unwrap();
    let store = Store::open(&root.path().join("data")).unwrap();
    let mut run = Run {
        id: "source-window".into(),
        name: "source-window".into(),
        status: "completed".into(),
        command: vec!["fixture".into()],
        cwd: project.to_string_lossy().into_owned(),
        runtime: "fixture".into(),
        started_at_us: 1,
        finished_at_us: Some(2),
        exit_code: Some(0),
        diagnostics: vec![],
        source: json!({}),
        correctness: json!({}),
    };
    store.save_run(&run).unwrap();
    let operations = Operations::new(store.clone());
    let current = operations
        .call(
            "source.read",
            json!({"run_id":run.id,"path":"large.ts","line":5001,"count":120}),
        )
        .await
        .unwrap();
    assert_source_contract(&current);
    assert_eq!(current["start_line"], 5001);
    assert_eq!(current["next_line"], 5121);
    assert_eq!(current["total_lines"], 6003);
    assert_eq!(current["identity_verified"], false);
    assert_eq!(current["build_mapping_verified"], false);
    assert!(
        current["content"]
            .as_str()
            .unwrap()
            .starts_with("line-5001\n")
    );

    let artifact = store
        .add_artifact(&run.id, &source, "source-snapshot")
        .unwrap();
    run.source = json!({"retained_sources":[{"path":"large.ts","artifact_id":artifact.id,"sha256":artifact.sha256}]});
    store.save_run(&run).unwrap();
    fs::write(&source, "the current file changed").unwrap();
    let retained = operations
        .call(
            "source.read",
            json!({"run_id":run.id,"path":"large.ts","line":6001,"count":120}),
        )
        .await
        .unwrap();
    assert_source_contract(&retained);
    assert_eq!(retained["content"], "line-6001\nline-6002\nline-6003");
    assert_eq!(retained["next_line"], Value::Null);
    assert_eq!(retained["total_lines"], 6003);
    assert_eq!(retained["identity_verified"], true);
    assert_eq!(retained["source_kind"], "recorded_snapshot");
    assert_eq!(retained["build_mapping_verified"], false);

    let beyond = operations
        .call(
            "source.read",
            json!({"run_id":run.id,"path":"large.ts","line":9007199254740991_u64}),
        )
        .await
        .unwrap();
    assert_eq!(beyond["content"], "");
    assert_eq!(beyond["next_line"], Value::Null);
    assert_eq!(beyond["total_lines"], 6003);
}

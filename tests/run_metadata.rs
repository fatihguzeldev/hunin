use hunin::{
    model::runs::Run,
    operations::{Operations, catalog},
    storage::{ReadBudgetExceeded, ReadLimits, Store},
};
use serde_json::json;

#[tokio::test]
async fn recording_metadata_does_not_require_aggregating_large_selected_payloads() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store
        .save_run(&Run {
            id: "large".into(),
            name: "large selected payload".into(),
            status: "completed".into(),
            command: vec![],
            cwd: ".".into(),
            runtime: "imported".into(),
            started_at_us: 1,
            finished_at_us: Some(1),
            exit_code: None,
            diagnostics: vec![],
            source: serde_json::Value::Null,
            correctness: serde_json::Value::Null,
        })
        .unwrap();
    store.accept("large", "input", &[json!({"schema_version":1,"kind":"event","id":"large-payload","run_id":"large","producer_id":"control","pid":0,"tid":"","timestamp_us":1,"basis":"explicit","name":"large","attributes":{"payload":"x".repeat(9*1024*1024)}})]).unwrap();
    let error = store
        .with_query_snapshot(
            "large",
            ReadLimits::default(),
            || Ok(()),
            |port| port.visit(&[], |_| Ok(())),
        )
        .unwrap_err();
    assert!(error.is::<ReadBudgetExceeded>());
    let operations = Operations::new(store);
    let aggregate = hunin::interfaces::mcp::handle(&operations,&json!({"jsonrpc":"2.0","id":0,"method":"tools/call","params":{"name":"runs.get","arguments":{"run_id":"large"}}}),true).await;
    assert_eq!(aggregate["result"]["isError"], true, "{aggregate}");
    let failure = &aggregate["result"]["_meta"]["hunin/operation-error"];
    assert_eq!(failure["code"], "work_budget_exceeded");
    assert_eq!(failure["budget"]["dimension"], "record_bytes");
    // The unrelated large event does not prevent a narrow metric query.
    let resources = operations
        .call("resources.query", json!({"run_id":"large"}))
        .await
        .unwrap();
    assert_eq!(resources["series"].as_array().unwrap().len(), 0);
    let response = hunin::interfaces::mcp::handle(&operations,&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"runs.metadata","arguments":{"run_id":"large"}}}),true).await;
    assert_eq!(response["result"]["isError"], false, "{response}");
    let metadata = &response["result"]["structuredContent"];
    assert_eq!(metadata["run"]["name"], "large selected payload");
    assert!(
        metadata["result_identity"]
            .as_str()
            .unwrap()
            .starts_with("selected-results-v1:")
    );
    assert!(metadata.get("summary").is_none());
    assert!(metadata.to_string().len() < 10_000);
    let definition = catalog()
        .iter()
        .find(|op| op["name"] == "runs.metadata")
        .unwrap();
    jsonschema::validator_for(&definition["outputSchema"])
        .unwrap()
        .validate(metadata)
        .unwrap();
}

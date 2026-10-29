use hunin::{
    model::runs::Run,
    operations::{Operations, catalog},
    storage::Store,
};
use serde_json::{Value, json};

fn validate_result(name: &str, result: &Value) {
    let definition = catalog()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap();
    jsonschema::validator_for(&definition["outputSchema"])
        .unwrap()
        .validate(result)
        .unwrap_or_else(|error| panic!("{name} response violates its output contract: {error}"));
}

fn validate_transport(name: &str, result: &Value) {
    let definitions: Value =
        serde_json::from_str(include_str!("../protocol/response.schema.json")).unwrap();
    let schema = json!({"$ref":format!("#/$defs/{name}"),"$defs":definitions["$defs"]});
    jsonschema::validator_for(&schema)
        .unwrap()
        .validate(result)
        .unwrap();
}

#[test]
fn published_output_schemas_resolve_without_external_files_or_network() {
    for operation in catalog() {
        if let Some(schema) = operation.get("outputSchema") {
            jsonschema::validator_for(schema)
                .unwrap_or_else(|error| panic!("{}: {error}", operation["name"]));
        }
    }
}

#[tokio::test]
async fn run_contract_preserves_extension_status_null_options_and_arbitrary_metadata() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    let run = Run {
        id: "legacy".into(),
        name: "legacy".into(),
        status: "archived-by-extension".into(),
        command: vec![],
        cwd: data.path().to_string_lossy().into_owned(),
        runtime: "imported".into(),
        started_at_us: 1,
        finished_at_us: None,
        exit_code: None,
        diagnostics: vec![],
        source: Value::Null,
        correctness: json!("unverified"),
    };
    store.save_run(&run).unwrap();
    let operations = Operations::new(store);
    let result = operations
        .call("runs.get", json!({"run_id":"legacy"}))
        .await
        .unwrap();
    validate_result("runs.get", &result);
    assert_eq!(result["run"], serde_json::to_value(&run).unwrap());
    let page = operations.call("runs.list", json!({})).await.unwrap();
    validate_result("runs.list", &page);
    assert_eq!(page["next_cursor"], Value::Null);

    let success = hunin::interfaces::mcp::handle(&operations, &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"runs.get","arguments":{"run_id":"legacy"}}}), true).await;
    validate_transport("McpToolSuccess", &success["result"]);
    validate_result("runs.get", &success["result"]["structuredContent"]);
    let failure = hunin::interfaces::mcp::handle(&operations, &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"runs.get","arguments":{"run_id":"missing"}}}), true).await;
    validate_transport("McpToolFailure", &failure["result"]);
    assert!(failure["result"].get("structuredContent").is_none());
}

#[test]
fn summary_contract_retains_non_finite_aggregate_as_null() {
    let sample =
        |id: &str| json!({"kind":"sample","id":id,"weight":f64::MAX,"weight_unit":"samples"});
    let summary = hunin::query::summary(&[sample("one"), sample("two")]);
    assert_eq!(summary["total_weight"], Value::Null);
    let definitions: Value =
        serde_json::from_str(include_str!("../protocol/response.schema.json")).unwrap();
    jsonschema::validator_for(&definitions["$defs"]["Summary"])
        .unwrap()
        .validate(&summary)
        .unwrap();
}

#[tokio::test]
async fn capabilities_and_source_contracts_cover_real_discovery_and_unverified_files() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let source = project.path().join("control.js");
    std::fs::write(&source, "function currentControl() {}\n").unwrap();
    let store = Store::open(data.path()).unwrap();
    store
        .save_run(&Run {
            id: "source".into(),
            name: "source".into(),
            status: "completed".into(),
            command: vec![],
            cwd: project.path().to_string_lossy().into_owned(),
            runtime: "imported".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: None,
            diagnostics: vec![],
            source: Value::Null,
            correctness: Value::Null,
        })
        .unwrap();
    let operations = Operations::new(store);
    let result = operations
        .call("source.read", json!({"run_id":"source","path":source}))
        .await
        .unwrap();
    validate_result("source.read", &result);
    assert_eq!(result["identity_verified"], false);
    assert_eq!(result["source_kind"], "current_file");
    let discovery = operations.call("capabilities", json!({})).await.unwrap();
    validate_result("capabilities", &discovery);
    assert_eq!(
        discovery["verification"],
        "Discovery never labels executable presence as a verified CPU recording."
    );
}

#[tokio::test]
async fn artifact_contracts_cover_real_pages_and_lossy_byte_excerpts() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store
        .save_run(&Run {
            id: "recording".into(),
            name: "recording".into(),
            status: "completed".into(),
            command: vec![],
            cwd: data.path().to_string_lossy().into_owned(),
            runtime: "imported".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: None,
            diagnostics: vec![],
            source: json!({"fingerprint":"unknown"}),
            correctness: json!({"passed":null}),
        })
        .unwrap();
    let artifact = store
        .add_artifact_bytes("recording", "utf8", "control", "AéB".as_bytes())
        .unwrap();
    store
        .add_artifact_bytes("recording", "other", "control", b"other")
        .unwrap();
    let operations = Operations::new(store);

    let page = operations
        .call("artifacts.list", json!({"run_id":"recording","limit":1}))
        .await
        .unwrap();
    validate_result("artifacts.list", &page);
    assert_eq!(page["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(page["next_cursor"], "1");
    let last = operations
        .call(
            "artifacts.list",
            json!({"run_id":"recording","limit":1,"cursor":"1"}),
        )
        .await
        .unwrap();
    validate_result("artifacts.list", &last);
    assert_eq!(last["next_cursor"], Value::Null);

    let excerpt = operations
        .call(
            "artifacts.read",
            json!({"run_id":"recording","artifact_id":artifact.id,"offset":2,"limit":1}),
        )
        .await
        .unwrap();
    validate_result("artifacts.read", &excerpt);
    assert_eq!(excerpt["content"], "�");
    assert_eq!(excerpt["encoding"], "utf8_lossy");
    assert_eq!(excerpt["next_offset"], 3);
    assert_eq!(excerpt["size"], 4);
    let end = operations
        .call(
            "artifacts.read",
            json!({"run_id":"recording","artifact_id":artifact.id,"offset":4}),
        )
        .await
        .unwrap();
    validate_result("artifacts.read", &end);
    assert_eq!(end["content"], "");
    assert_eq!(end["next_offset"], Value::Null);
}

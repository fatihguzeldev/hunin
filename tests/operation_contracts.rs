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

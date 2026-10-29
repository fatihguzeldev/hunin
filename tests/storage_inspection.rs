use hunin::{
    model::runs::Run,
    operations::{Operations, catalog},
    storage::Store,
};
use serde_json::{Value, json};

fn fixture_run() -> Run {
    Run {
        id: "inspection-control".into(),
        name: "Inspection control".into(),
        status: "completed".into(),
        command: vec![],
        cwd: ".".into(),
        runtime: "imported".into(),
        started_at_us: 1,
        finished_at_us: Some(1),
        exit_code: None,
        diagnostics: vec![],
        source: Value::Null,
        correctness: Value::Null,
    }
}

fn validate(result: &Value) {
    let operation = catalog()
        .iter()
        .find(|op| op["name"] == "storage.inspect")
        .unwrap();
    jsonschema::validator_for(&operation["outputSchema"])
        .unwrap()
        .validate(result)
        .unwrap();
}

#[tokio::test]
async fn inspection_reports_uncertain_ownership_and_missing_bytes_without_repair() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store.save_run(&fixture_run()).unwrap();
    let artifact = store
        .add_artifact_bytes(
            "inspection-control",
            "control.txt",
            "text",
            b"retained bytes",
        )
        .unwrap();
    std::fs::remove_file(data.path().join("artifacts").join(&artifact.sha256)).unwrap();
    let orphan = data.path().join("artifacts").join("a".repeat(64));
    let pending = data.path().join("artifacts/.pending-active-control");
    let workspace = data.path().join("captures/unknown-owner");
    std::fs::write(&orphan, b"not committed yet").unwrap();
    std::fs::write(&pending, b"active writer").unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let operations = Operations::new(store.clone());
    let result = operations.call("storage.inspect", json!({})).await.unwrap();
    validate(&result);
    assert_eq!(result["deletion_authorized"], false);
    assert_eq!(result["content_hashes_checked"], 0);
    assert_eq!(result["filesystem_view"], "live_metadata");
    let findings = result["findings"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .any(|finding| finding["kind"] == "missing_referenced_blob"
                && finding["sha256"] == artifact.sha256)
    );
    let unknown = findings
        .iter()
        .find(|finding| finding["kind"] == "unreferenced_at_snapshot")
        .unwrap();
    assert_eq!(unknown["size_bytes"], "17");
    assert!(
        findings
            .iter()
            .any(|finding| finding["kind"] == "pending_ownership_unknown")
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding["kind"] == "workspace_ownership_unknown"
                && finding["metadata_reference"] == "absent")
    );
    assert_eq!(std::fs::read(&orphan).unwrap(), b"not committed yet");
    assert_eq!(std::fs::read(&pending).unwrap(), b"active writer");
    assert!(workspace.is_dir());
    assert_eq!(store.artifacts("inspection-control").unwrap().len(), 1);
}

#[tokio::test]
async fn inspection_exposes_actual_budgets_and_limited_coverage() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    std::fs::write(data.path().join("artifacts/.pending-one"), b"one").unwrap();
    std::fs::write(data.path().join("artifacts/.pending-two"), b"two").unwrap();
    let operations = Operations::new(store);
    let result = operations
        .call(
            "storage.inspect",
            json!({"finding_limit":1.0,"filesystem_entry_limit":1,"budget_ms":1000}),
        )
        .await
        .unwrap();
    validate(&result);
    assert_eq!(result["budgets"]["finding_limit"], 1);
    assert_eq!(result["findings"].as_array().unwrap().len(), 1);
    assert!(!result["limits_hit"].as_array().unwrap().is_empty());
    assert!(
        result["scans"]
            .as_array()
            .unwrap()
            .iter()
            .any(|scan| scan["status"] == "limited")
    );
    for params in [
        json!({"finding_limit":0}),
        json!({"filesystem_entry_limit":10001}),
        json!({"artifact_row_limit":10001}),
        json!({"budget_ms":5001}),
        json!({"repair":true}),
    ] {
        assert!(operations.call("storage.inspect", params).await.is_err());
    }
}

#[tokio::test]
async fn inspection_has_one_contract_through_mcp_with_explicit_read_lifecycle() {
    let data = tempfile::tempdir().unwrap();
    let operations = Operations::new(Store::open(data.path()).unwrap());
    let response = hunin::interfaces::mcp::handle(&operations, &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"storage.inspect","arguments":{}}}), true).await;
    validate(&response["result"]["structuredContent"]);
    let operation = catalog()
        .iter()
        .find(|op| op["name"] == "storage.inspect")
        .unwrap();
    assert_eq!(operation["_meta"]["hunin/lifecycle"]["kind"], "read");
}

// APFS rejects non-Unicode filenames before inspection can observe them.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn inspection_projects_non_unicode_paths_as_display_without_action_authority() {
    use std::os::unix::ffi::OsStringExt;
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    let name = std::ffi::OsString::from_vec(b".pending-\xff-owner".to_vec());
    let path = data.path().join("artifacts").join(name);
    std::fs::write(&path, b"owned bytes").unwrap();
    let operations = Operations::new(store);
    let result = operations.call("storage.inspect", json!({})).await.unwrap();
    validate(&result);
    assert_eq!(result["path_encoding"], "lossy_display_only");
    assert_eq!(result["deletion_authorized"], false);
    let finding = result["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["kind"] == "unfollowed_path")
        .unwrap();
    assert!(
        finding["relative_path"]
            .as_str()
            .unwrap()
            .contains('\u{fffd}')
    );
    assert_eq!(std::fs::read(path).unwrap(), b"owned bytes");
}

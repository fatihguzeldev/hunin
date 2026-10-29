use hunin::{
    model::runs::Run,
    operations::{Operations, catalog},
    storage::Store,
};
use serde_json::{Value, json};

#[tokio::test]
async fn removal_receipts_are_readable_after_deletion_and_do_not_authorize_unknown_versions() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let run:Run=serde_json::from_value(json!({"id":"removed","name":"receipt","status":"completed","command":[],"cwd":".","runtime":"none","started_at_us":1,"finished_at_us":2,"exit_code":0,"diagnostics":[],"source":{},"correctness":{}})).unwrap();
    store.save_run(&run).unwrap();
    let operations = Operations::new(store.clone());
    store
        .save_document(
            "analysis-results",
            "active",
            &json!({"id":"active","run_id":"removed","status":"running"}),
        )
        .unwrap();
    let conflict = hunin::interfaces::mcp::handle(&operations,&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"runs.remove","arguments":{"run_id":"removed"}}}),true).await;
    assert_eq!(
        conflict["result"]["_meta"]["hunin/operation-error"]["code"],
        "conflict"
    );
    assert!(store.run("removed").is_ok());
    assert!(store.removal_receipt("removed").unwrap().is_none());
    store
        .save_document(
            "analysis-results",
            "active",
            &json!({"id":"active","run_id":"removed","status":"canceled"}),
        )
        .unwrap();
    assert_eq!(
        operations
            .call("runs.removal", json!({"run_id":"missing"}))
            .await
            .unwrap(),
        json!({"receipt":null})
    );
    assert_eq!(
        operations
            .call("runs.remove", json!({"run_id":"removed"}))
            .await
            .unwrap(),
        json!({"removed":true})
    );
    let result = operations
        .call("runs.removal", json!({"run_id":"removed"}))
        .await
        .unwrap();
    assert_eq!(result["receipt"]["completed"], true);
    assert_eq!(result["receipt"]["run_id"], "removed");
    let definition = catalog()
        .iter()
        .find(|op| op["name"] == "runs.removal")
        .unwrap();
    jsonschema::validator_for(&definition["outputSchema"])
        .unwrap()
        .validate(&result)
        .unwrap();
    assert_eq!(
        operations
            .call("runs.remove", json!({"run_id":"removed"}))
            .await
            .unwrap(),
        json!({"removed":true})
    );
    let future = json!({"version":99,"run_id":"future","unknown":{"preserved":[null,7]}});
    store
        .save_document("run-removals", "future", &future)
        .unwrap();
    let before = store.document("run-removals", "future").unwrap();
    let result = operations
        .call("runs.removal", json!({"run_id":"future"}))
        .await
        .unwrap();
    assert_eq!(result["receipt"], future);
    jsonschema::validator_for(&definition["outputSchema"])
        .unwrap()
        .validate(&result)
        .unwrap();
    assert!(
        operations
            .call("runs.remove", json!({"run_id":"future"}))
            .await
            .is_err()
    );
    assert_eq!(store.document("run-removals", "future").unwrap(), before);
    assert!(
        result["receipt"]
            .as_object()
            .unwrap()
            .get("completed")
            .unwrap_or(&Value::Null)
            .is_null()
    );
    operations.shutdown().await.unwrap();
}

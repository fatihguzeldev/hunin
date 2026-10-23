use hunin::{model::runs::Run, operations::Operations, storage::Store};
use serde_json::json;

fn run(id: &str, cwd: &str, passed: bool) -> Run {
    Run {
        id: id.into(),
        name: id.into(),
        status: "completed".into(),
        command: vec!["workload".into()],
        cwd: cwd.into(),
        runtime: "none".into(),
        started_at_us: 10,
        finished_at_us: Some(30),
        exit_code: Some(if passed { 0 } else { 1 }),
        diagnostics: vec![],
        source: json!({"fingerprint":"known"}),
        correctness: json!({"passed":passed}),
    }
}

#[tokio::test]
async fn source_urls_are_decoded_but_cannot_escape_the_recorded_project() {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store
        .save_run(&run("r", root.path().to_str().unwrap(), true))
        .unwrap();
    let source = root.path().join("a b.js");
    std::fs::write(&source, "function hotspot() {}\n").unwrap();
    let ops = Operations::new(store);
    let file_url = url::Url::from_file_path(&source).unwrap().to_string();
    let response = ops
        .call("source.read", json!({"run_id":"r","path":file_url}))
        .await
        .unwrap();
    assert!(response["content"].as_str().unwrap().contains("hotspot"));
    assert_eq!(response["identity_verified"], false);
    let outside = data.path().join("outside.js");
    std::fs::write(&outside, "outside").unwrap();
    assert!(
        ops.call("source.read", json!({"run_id":"r","path":outside}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn recorded_source_survives_edits_and_project_removal_without_claiming_build_mapping() {
    let project = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let source = project.path().canonicalize().unwrap().join("app.js");
    std::fs::write(
        &source,
        "function recordedHotspot() {}\nrecordedHotspot();\n",
    )
    .unwrap();
    let ops = Operations::new(Store::open(data.path()).unwrap());
    let started = ops
        .call(
            "runs.start",
            json!({"command":["node","app.js"],"runtime":"none","cwd":project.path()}),
        )
        .await
        .unwrap();
    let id = started["run"]["id"].as_str().unwrap();
    let finished = ops.engine.wait(id).await.unwrap();
    assert_eq!(finished.status, "completed");
    std::fs::write(&source, "function changedHotspot() {}\n").unwrap();
    let result = ops
        .call("source.read", json!({"run_id":id,"path":source}))
        .await
        .unwrap();
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .contains("recordedHotspot")
    );
    assert_eq!(result["identity_verified"], true);
    assert_eq!(result["build_mapping_verified"], false);
    project.close().unwrap();
    let reopened = Operations::new(Store::open(data.path()).unwrap());
    assert_eq!(
        reopened
            .call("source.read", json!({"run_id":id,"path":source}))
            .await
            .unwrap()["content"],
        result["content"]
    );
}

#[tokio::test]
async fn incorrect_output_never_gets_an_unqualified_optimization_verdict() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store.save_run(&run("before", "/tmp", true)).unwrap();
    store.save_run(&run("after", "/tmp", false)).unwrap();
    let result = Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    assert_eq!(result["comparable"], false);
    assert!(!result["reasons"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn removed_hotspots_are_present_as_zero_after_and_missing_identity_is_rejected() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    let mut a = run("before", "/tmp", true);
    a.source["collector_settings_fingerprint"] = json!("same");
    let mut b = a.clone();
    b.id = "after".into();
    store.save_run(&a).unwrap();
    store.save_run(&b).unwrap();
    let sample = json!({"schema_version":1,"kind":"sample","id":"removed","run_id":"before","producer_id":"p","pid":1,"tid":"1","timestamp_us":10,"basis":"sampled","stack":[{"function":"removedHotspot"}],"weight":3,"weight_unit":"samples"});
    store.accept("before", "samples", &[sample]).unwrap();
    let result = Operations::new(store.clone())
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    let removed = result["hotspots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["function"] == "removedHotspot")
        .unwrap();
    assert_eq!(removed["after"], 0.0);
    b.source["fingerprint"] = serde_json::Value::Null;
    store.save_run(&b).unwrap();
    let result = Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    assert_eq!(result["comparable"], false);
}

#[tokio::test]
async fn mcp_rejects_non_scalar_request_ids_before_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let ops = Operations::new(Store::open(root.path()).unwrap());
    let response = hunin::interfaces::mcp::handle(
        &ops,
        &json!({"jsonrpc":"2.0","id":{},"method":"ping"}),
        true,
    )
    .await;
    assert_eq!(response["error"]["code"], -32600);
}

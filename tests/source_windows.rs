use hunin::{
    model::runs::Run,
    operations::{Operations, catalog},
    storage::Store,
};
use serde_json::{Value, json};
use std::fs;

fn recorded_run(id: &str, cwd: &str) -> Run {
    Run {
        id: id.into(),
        name: id.into(),
        status: "completed".into(),
        command: vec!["workload".into()],
        cwd: cwd.into(),
        runtime: "none".into(),
        started_at_us: 10,
        finished_at_us: Some(30),
        exit_code: Some(0),
        diagnostics: vec![],
        source: json!({"fingerprint":"known","collector_settings_fingerprint":"settings","environment_fingerprint":"environment","workload_inputs":{"fingerprint":"inputs"},"correctness_recipe_fingerprint":"recipe","supervised_elapsed_us":10,"supervised_execution_scope":"target_process"}),
        correctness: json!({"passed":true}),
    }
}

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

#[tokio::test]
async fn source_urls_are_decoded_but_cannot_escape_the_recorded_project() {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store
        .save_run(&recorded_run("r", root.path().to_str().unwrap()))
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
    let error = ops
        .call("source.read", json!({"run_id":"r","path":outside}))
        .await
        .unwrap_err();
    let failure = hunin::operations::describe_failure("source.read", &error);
    assert_eq!(failure["code"], "source_outside_project");
    assert_eq!(failure["operation"], "source.read");
    assert_eq!(
        failure["message"],
        "source path is outside the measured project"
    );
    let schema: Value =
        serde_json::from_str(include_str!("../protocol/response.schema.json")).unwrap();
    jsonschema::validator_for(&schema["$defs"]["OperationFailure"])
        .unwrap()
        .validate(&failure)
        .unwrap();
    let mcp = hunin::interfaces::mcp::handle(
        &ops,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"source.read","arguments":{"run_id":"r","path":outside}}}),
        true,
    )
    .await;
    assert_eq!(mcp["result"]["_meta"]["hunin/operation-error"], failure);
    let lookalike = anyhow::anyhow!("source path is outside the measured project");
    assert_eq!(
        hunin::operations::describe_failure("source.read", &lookalike)["code"],
        "operation_failed"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn retained_source_resolves_existing_parent_alias_after_file_removal() {
    let directory = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let alias = directory.path().join("alias");
    std::os::unix::fs::symlink(&project, &alias).unwrap();
    let source = project.join("app.js");
    std::fs::write(&source, "function retainedHotspot() {}\n").unwrap();
    let store = Store::open(data.path()).unwrap();
    let mut recorded = recorded_run(
        "alias-source",
        project.canonicalize().unwrap().to_str().unwrap(),
    );
    store.save_run(&recorded).unwrap();
    let artifact = store
        .add_artifact(&recorded.id, &source, "source-snapshot")
        .unwrap();
    recorded.source["retained_sources"] =
        json!([{"path":"app.js","artifact_id":artifact.id,"sha256":artifact.sha256}]);
    store.save_run(&recorded).unwrap();
    std::fs::remove_file(&source).unwrap();
    let ops = Operations::new(store);
    let response = ops
        .call(
            "source.read",
            json!({"run_id":recorded.id,"path":alias.join("app.js")}),
        )
        .await
        .unwrap();
    assert_eq!(response["content"], "function retainedHotspot() {}");
    assert_eq!(response["source_kind"], "recorded_snapshot");
    let outside = directory.path().join("outside");
    std::os::unix::fs::symlink(data.path(), &outside).unwrap();
    assert!(
        ops.call(
            "source.read",
            json!({"run_id":recorded.id,"path":outside.join("app.js")})
        )
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
    // Runtime URLs use ordinary DOS paths while canonical recording roots may
    // use Win32 extended-length prefixes. Removed sources must still match.
    #[cfg(windows)]
    let source = source
        .to_string_lossy()
        .trim_start_matches("\\\\?\\")
        .to_owned();
    assert_eq!(
        reopened
            .call("source.read", json!({"run_id":id,"path":source}))
            .await
            .unwrap()["content"],
        result["content"]
    );
}

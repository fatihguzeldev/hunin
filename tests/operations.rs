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
        source: json!({"fingerprint":"known","collector_settings_fingerprint":"settings","environment_fingerprint":"environment","workload_inputs":{"fingerprint":"inputs"},"correctness_recipe_fingerprint":"recipe","supervised_elapsed_us":10,"supervised_execution_scope":"target_process"}),
        correctness: json!({"passed":passed}),
    }
}

#[tokio::test]
async fn comparison_accepts_source_changes_but_rejects_changed_inputs_environment_and_recipe() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    let before = run("before", "/project", true);
    let mut after = before.clone();
    after.id = "after".into();
    after.source["fingerprint"] = json!("optimized-source");
    store.save_run(&before).unwrap();
    store.save_run(&after).unwrap();
    let ops = Operations::new(store.clone());
    let result = ops
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    assert_eq!(result["comparable"], true, "{result}");
    assert_eq!(result["metrics"][0]["scope"], "recording_orchestration");
    assert_eq!(result["metrics"][1]["scope"], "target_process");
    for (field, value, reason) in [
        (
            "environment_fingerprint",
            json!("changed-env"),
            "environment",
        ),
        (
            "workload_inputs",
            json!({"fingerprint":"changed-input"}),
            "input",
        ),
        (
            "correctness_recipe_fingerprint",
            json!("changed-recipe"),
            "correctness",
        ),
    ] {
        let original = after.source[field].clone();
        after.source[field] = value;
        store.save_run(&after).unwrap();
        let result = ops
            .call(
                "comparisons.create",
                json!({"before":"before","after":"after"}),
            )
            .await
            .unwrap();
        assert_eq!(result["comparable"], false, "{field}");
        assert!(
            result["reasons"].as_array().unwrap().iter().any(|v| v
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains(reason)),
            "{result}"
        );
        after.source[field] = original;
    }
}

#[tokio::test]
async fn stored_scenario_replays_environment_inputs_and_stdout_recipe() {
    let project = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("input.txt"), "payload").unwrap();
    let source = "const fs=require('node:fs');process.stdout.write(process.env.SCENARIO_LABEL+':'+fs.readFileSync('input.txt','utf8'));";
    std::fs::write(project.path().join("app.cjs"), source).unwrap();
    let ops = Operations::new(Store::open(data.path()).unwrap());
    let saved = ops.call("scenarios.save",json!({"name":"checked input","command":["node","app.cjs"],"runtime":"none","cwd":project.path(),"env":{"SCENARIO_LABEL":"case"},"input_files":["input.txt"],"expected_stdout":"case:payload"})).await.unwrap();
    let id = saved["scenario"]["id"].as_str().unwrap();
    let started = ops
        .call("scenarios.run", json!({"scenario_id":id}))
        .await
        .unwrap();
    let run = ops
        .engine
        .wait(started["run"]["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(run.correctness["passed"], true);
    assert_eq!(run.correctness["validation"], "exit_code_and_stdout");
    assert_eq!(
        run.source["environment"]["overrides"]["SCENARIO_LABEL"],
        "case"
    );
    assert_eq!(
        run.source["workload_inputs"]["files"][0]["path"],
        "input.txt"
    );
    let listed = ops.call("scenarios.list", json!({})).await.unwrap();
    assert_eq!(listed["scenarios"][0]["expected_stdout"], "case:payload");
}

#[tokio::test]
async fn resource_comparisons_deduplicate_process_lifetimes_and_do_not_extrapolate_startup_points()
{
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store.save_run(&run("before", "/project", true)).unwrap();
    store.save_run(&run("after", "/project", true)).unwrap();
    let point = |id: &str,
                 run: &str,
                 producer: &str,
                 time: u64,
                 name: &str,
                 value: f64,
                 scope: &str,
                 role: &str| json!({"schema_version":1,"kind":"metric","id":id,"run_id":run,"producer_id":producer,"pid":1,"tid":"process","timestamp_us":time,"basis":"observed","name":name,"value":value,"unit":if name=="process.rss"{"bytes"}else{"milliseconds"},"scope":scope,"attributes":{"process_started_at_seconds":100,"process_role":role,"temporality":if name=="process.rss"{"gauge"}else{"cumulative"}}});
    let before = vec![
        point(
            "c1",
            "before",
            "p1",
            10,
            "process.cpu_time",
            10.,
            "process",
            "measured",
        ),
        point(
            "c2",
            "before",
            "p1",
            20,
            "process.cpu_time",
            20.,
            "process",
            "measured",
        ),
        point(
            "duplicate",
            "before",
            "p2",
            20,
            "process.cpu_time",
            20.,
            "process",
            "measured",
        ),
        point(
            "collector",
            "before",
            "wrapper",
            20,
            "process.cpu_time",
            10000.,
            "process",
            "collector",
        ),
        point(
            "context",
            "before",
            "sdk",
            20,
            "process.cpu_time",
            20000.,
            "context",
            "measured",
        ),
        point(
            "r1",
            "before",
            "p1",
            10,
            "process.rss",
            100.,
            "process",
            "measured",
        ),
        point(
            "r2",
            "before",
            "p1",
            20,
            "process.rss",
            200.,
            "process",
            "measured",
        ),
    ];
    let after = vec![
        point(
            "c3",
            "after",
            "p1",
            10,
            "process.cpu_time",
            0.,
            "process",
            "measured",
        ),
        point(
            "r3",
            "after",
            "p1",
            10,
            "process.rss",
            5.,
            "process",
            "measured",
        ),
    ];
    store.accept("before", "resources", &before).unwrap();
    store.accept("after", "resources", &after).unwrap();
    let result = Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    let cpu = result["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "process.cpu_time.observed_total")
        .unwrap();
    assert_eq!(cpu["before"], 20.0);
    assert_eq!(cpu["after"], 0.0);
    assert_eq!(cpu["measurement_quality"], "insufficient_observations");
    assert_eq!(cpu["delta"], serde_json::Value::Null);
    assert_eq!(cpu["percent_change"], serde_json::Value::Null);
    let rss = result["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "process.rss.max_process")
        .unwrap();
    assert_eq!(rss["before"], 200.0);
    assert_eq!(rss["percent_change"], serde_json::Value::Null);
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

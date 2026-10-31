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
async fn binary_artifact_excerpts_preserve_exact_bytes_and_byte_offsets() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store.save_run(&run("recording", "/project", true)).unwrap();
    let bytes = (0u8..=255).collect::<Vec<_>>();
    let artifact = store
        .add_artifact_bytes("recording", "machine-code.bin", "control", &bytes)
        .unwrap();
    let operations = Operations::new(store);
    let mut offset = 0;
    let mut restored = vec![];
    loop {
        let page=operations.call("artifacts.read",json!({"run_id":"recording","artifact_id":artifact.id,"offset":offset,"limit":73,"encoding":"hex"})).await.unwrap();
        assert_eq!(page["encoding"], "hex");
        let content = page["content"].as_str().unwrap();
        assert!(content.len() <= 146);
        assert_eq!(content.len() % 2, 0);
        restored.extend(
            (0..content.len())
                .step_by(2)
                .map(|index| u8::from_str_radix(&content[index..index + 2], 16).unwrap()),
        );
        assert_eq!(page["size"], 256);
        let Some(next) = page["next_offset"].as_u64() else {
            break;
        };
        assert_eq!(next, restored.len() as u64);
        offset = next;
    }
    assert_eq!(restored, bytes);
}

#[tokio::test]
async fn evidence_and_context_pages_filter_before_decoding_unrelated_observations() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.save_run(&run("recording", "/project", true)).unwrap();
    store.save_run(&run("foreign", "/project", true)).unwrap();
    let context = |id, time| json!({"schema_version":1,"kind":"context","id":id,"run_id":"recording","producer_id":"control","pid":1,"tid":"main","timestamp_us":time,"basis":"observed","name":id});
    let event = json!({"schema_version":1,"kind":"event","id":"unrelated","run_id":"recording","producer_id":"control","pid":1,"tid":"main","timestamp_us":1,"basis":"observed","name":"unrelated"});
    let first = context("first", 2);
    let second = context("second", 4);
    let relation = json!({"schema_version":1,"kind":"relation","id":"link","run_id":"recording","producer_id":"control","pid":1,"tid":"main","timestamp_us":3,"basis":"observed","from":"first","to":"second","relation":"links","evidence":["first","second"]});
    store
        .accept(
            "recording",
            "input",
            &[event, first.clone(), relation.clone(), second.clone()],
        )
        .unwrap();
    let mut foreign = first.clone();
    foreign["run_id"] = json!("foreign");
    store
        .accept("foreign", "foreign-input", &[foreign])
        .unwrap();
    let connection = rusqlite::Connection::open(directory.path().join("hunin.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE observations SET record='not JSON' WHERE run_id='recording' AND id='unrelated'",
            [],
        )
        .unwrap();
    let operations = Operations::new(store);
    let evidence = operations
        .call(
            "evidence.get",
            json!({"run_id":"recording","ids":["second","first","first","missing"]}),
        )
        .await
        .unwrap();
    assert_eq!(evidence["records"], json!([first.clone(), second.clone()]));
    for (cursor, contexts, relations, next) in [
        ("0", json!([first]), json!([]), json!("1")),
        ("1", json!([]), json!([relation]), json!("2")),
        ("2", json!([second]), json!([]), json!(null)),
    ] {
        let page = operations
            .call(
                "contexts.query",
                json!({"run_id":"recording","cursor":cursor,"limit":1}),
            )
            .await
            .unwrap();
        assert_eq!(
            page,
            json!({"contexts":contexts,"relations":relations,"next_cursor":next})
        );
    }
}

#[tokio::test]
async fn analysis_without_result_identity_is_never_selected() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store.save_run(&run("recording", "/project", true)).unwrap();
    // Historical metadata can lack both result_set_id and id. The document key
    // still owns the metadata; an absent selected result must not match it.
    store
        .save_document(
            "analysis-results",
            "legacy-key",
            &json!({"run_id":"recording","analysis_id":"legacy","status":"failed"}),
        )
        .unwrap();
    let results = Operations::new(store)
        .call("analyses.results", json!({"run_id":"recording"}))
        .await
        .unwrap();
    assert_eq!(results["results"][0]["selected"], false);
}

#[tokio::test]
async fn exact_analysis_attempt_lookup_preserves_replay_identity_and_run_scope() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store.save_run(&run("recording", "/project", true)).unwrap();
    store.save_run(&run("other", "/project", true)).unwrap();
    store.save_document("analysis-results", "attempt", &json!({"id":"attempt","run_id":"recording","analysis_id":"analyzer","result_set_id":"replayed-result","status":"replayed","created_at_us":0})).unwrap();
    for index in 1..4 {
        store.save_document("analysis-results", &format!("newer-{index}"), &json!({"id":format!("newer-{index}"),"run_id":"recording","status":"failed","created_at_us":index})).unwrap();
    }
    store
        .save_document(
            "analysis-results",
            "foreign",
            &json!({"id":"foreign","run_id":"other","status":"accepted"}),
        )
        .unwrap();
    let operations = Operations::new(store);
    let page = operations
        .call("analyses.results", json!({"run_id":"recording","limit":2}))
        .await
        .unwrap();
    assert!(
        page["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["id"] != "attempt")
    );
    let result = operations
        .call(
            "analyses.results",
            json!({"run_id":"recording","execution_id":"attempt","limit":2}),
        )
        .await
        .unwrap();
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    assert_eq!(result["results"][0]["id"], "attempt");
    assert_eq!(result["results"][0]["result_set_id"], "replayed-result");
    assert!(result["next_cursor"].is_null());
    for params in [
        json!({"run_id":"recording","execution_id":"foreign"}),
        json!({"run_id":"recording","execution_id":"missing"}),
        json!({"run_id":"recording","execution_id":"attempt","cursor":"1"}),
    ] {
        let result = operations.call("analyses.results", params).await.unwrap();
        assert!(result["results"].as_array().unwrap().is_empty());
        assert!(result["next_cursor"].is_null());
    }
}

#[tokio::test]
async fn schema_integer_parameters_accept_integral_json_float_representations() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store.save_run(&run("recording", "/project", true)).unwrap();
    let artifact = store
        .add_artifact_bytes("recording", "bytes.txt", "raw", b"0123456789")
        .unwrap();
    let result = Operations::new(store)
        .call(
            "artifacts.read",
            json!({"run_id":"recording","artifact_id":artifact.id,"offset":2.0,"limit":3.0}),
        )
        .await
        .unwrap();
    assert_eq!(result["content"], "234");
    assert_eq!(result["next_offset"], 5);
}

#[tokio::test]
async fn durable_metadata_lists_page_without_losing_history_or_crossing_runs() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store.save_run(&run("recording", "/project", true)).unwrap();
    store.save_run(&run("other", "/project", true)).unwrap();
    for index in 0..3 {
        let id = format!("item-{index}");
        for namespace in ["analyses", "scenarios"] {
            store
                .save_document(
                    namespace,
                    &id,
                    &json!({"id":id,"name":id,"created_at_us":index}),
                )
                .unwrap();
        }
        store.save_document("analysis-results", &id, &json!({"id":id,"run_id":"recording","analysis_id":"analyzer","created_at_us":index,"status":"failed"})).unwrap();
        store
            .add_artifact_bytes("recording", &id, "control", id.as_bytes())
            .unwrap();
    }
    store
        .save_document(
            "analysis-results",
            "foreign",
            &json!({"id":"foreign","run_id":"other","status":"failed","created_at_us":99}),
        )
        .unwrap();
    store
        .add_artifact_bytes("other", "foreign", "control", b"other run")
        .unwrap();
    let ops = Operations::new(store);
    for (operation, key, owned) in [
        ("analyses.list", "analyses", false),
        ("scenarios.list", "scenarios", false),
        ("analyses.results", "results", true),
        ("artifacts.list", "artifacts", true),
    ] {
        let mut params = json!({"limit":2});
        if owned {
            params["run_id"] = json!("recording");
        }
        let first = ops.call(operation, params.clone()).await.unwrap();
        assert_eq!(first[key].as_array().unwrap().len(), 2, "{operation}");
        assert_eq!(first["next_cursor"], "2", "{operation}");
        params["cursor"] = first["next_cursor"].clone();
        let second = ops.call(operation, params).await.unwrap();
        assert_eq!(second[key].as_array().unwrap().len(), 1, "{operation}");
        assert!(second["next_cursor"].is_null(), "{operation}");
        let ids: std::collections::HashSet<_> = first[key]
            .as_array()
            .unwrap()
            .iter()
            .chain(second[key].as_array().unwrap())
            .map(|row| row["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), 3, "{operation}: pages repeated an item");
        assert!(
            !ids.contains("foreign"),
            "{operation}: crossed run ownership"
        );
        let mut invalid = json!({"limit":501});
        if owned {
            invalid["run_id"] = json!("recording");
        }
        assert!(
            ops.call(operation, invalid).await.is_err(),
            "{operation}: accepted an unbounded page"
        );
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
async fn absent_sample_units_remain_unavailable_and_missing_identity_is_rejected() {
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
    assert_eq!(removed["after"], serde_json::Value::Null);
    assert_eq!(removed["after_available"], false);
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

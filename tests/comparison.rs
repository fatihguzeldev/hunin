use hunin::{model::runs::Run, operations::Operations, storage::Store};
use serde_json::{Value, json};

fn recording(id: &str) -> Run {
    Run {
        id: id.into(),
        name: id.into(),
        status: "completed".into(),
        command: vec!["workload".into()],
        cwd: "/project".into(),
        runtime: "none".into(),
        started_at_us: 10,
        finished_at_us: Some(30),
        exit_code: Some(0),
        diagnostics: vec![],
        source: json!({
            "fingerprint":"known-source",
            "collector_settings_fingerprint":"same-settings",
            "environment_fingerprint":"same-environment",
            "workload_inputs":{"fingerprint":"same-inputs"},
            "correctness_recipe_fingerprint":"same-recipe",
            "supervised_elapsed_us":10,
            "supervised_execution_scope":"target_process"
        }),
        correctness: json!({"passed":true}),
    }
}

#[tokio::test]
async fn metric_availability_is_distinct_from_measurement_and_workload_checks() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&recording("before")).unwrap();
    let mut after = recording("after");
    after.command.push("different workload".into());
    store.save_run(&after).unwrap();
    let result = Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    let row = metric(&result, "recording.elapsed");
    assert_eq!(result["comparable"], false);
    assert_eq!(row["values_available"], true);
    assert_eq!(row["measurement_comparable"], true);
    assert_eq!(row["workload_comparable"], false);
    assert_eq!(row["comparable"], row["measurement_comparable"]);
}

#[tokio::test]
async fn overflowing_process_totals_are_not_comparable_numeric_measurements() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    for id in ["before", "after"] {
        store.save_run(&recording(id)).unwrap();
        let rows = (1..=2).flat_map(|pid| (1..=2).map(move |timestamp| json!({"schema_version":1,"kind":"metric","id":format!("{id}-{pid}-{timestamp}"),"run_id":id,"producer_id":"control","pid":pid,"tid":"","timestamp_us":timestamp,"basis":"observed","name":"process.cpu_time","value":1e308,"unit":"milliseconds","scope":"process","attributes":{"process_lifetime_id":format!("process-{pid}"),"temporality":"cumulative"}}))).collect::<Vec<_>>();
        store.accept(id, "control", &rows).unwrap();
    }
    let result = Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    let row = metric(&result, "process.cpu_time.observed_total");
    assert_eq!(row["values_available"], false);
    assert_eq!(row["measurement_comparable"], false);
    assert_eq!(row["comparable"], false);
    assert_eq!(row["delta"], Value::Null);
}

#[tokio::test]
async fn declared_input_path_changes_do_not_claim_content_changes() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    for (id, path) in [("before", "old/input.bin"), ("after", "moved/input.bin")] {
        let mut run = recording(id);
        run.source["workload_inputs"] =
            json!({"fingerprint":id,"files":[{"path":path,"sha256":"a".repeat(64),"bytes":4}]});
        store.save_run(&run).unwrap();
    }
    let operations = Operations::new(store.clone());
    let params = json!({"before":"before","after":"after"});
    let path_change = operations
        .call("comparisons.create", params.clone())
        .await
        .unwrap();
    assert_eq!(path_change["comparable"], false);
    assert!(
        path_change["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap().contains("paths differ"))
    );
    let mut after = store.run("after").unwrap();
    after.source["workload_inputs"]["files"][0]["sha256"] = json!("b".repeat(64));
    store.save_run(&after).unwrap();
    let content_change = operations.call("comparisons.create", params).await.unwrap();
    assert!(
        content_change["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap().contains("content digests differ"))
    );
}

fn sample(run: &str, function: &str, unit: &str, weight: f64) -> Value {
    json!({"schema_version":1,"kind":"sample","id":format!("{run}-{function}-{unit}"),"run_id":run,"producer_id":"control","pid":1,"tid":"main","timestamp_us":10,"basis":"sampled","stack":[{"function":function}],"weight":weight,"weight_unit":unit})
}

fn hotspot<'a>(result: &'a Value, function: &str) -> &'a Value {
    result["hotspots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["function"] == function)
        .unwrap()
}

fn metric<'a>(result: &'a Value, name: &str) -> &'a Value {
    result["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap()
}

#[tokio::test]
async fn missing_selected_sample_unit_is_unknown_instead_of_measured_zero() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    for run in ["before", "after"] {
        store.save_run(&recording(run)).unwrap();
    }
    store
        .accept(
            "before",
            "input",
            &[sample("before", "allocated", "bytes", 4.)],
        )
        .unwrap();
    store
        .accept(
            "after",
            "input",
            &[sample("after", "sampled", "samples", 2.)],
        )
        .unwrap();
    let result = Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    let allocated = hotspot(&result, "allocated");
    assert_eq!(allocated["before"], 4.);
    assert_eq!(allocated["after"], Value::Null);
    assert_eq!(allocated["delta"], Value::Null);
    assert_eq!(allocated["after_available"], false);
    assert_eq!(allocated["presence"], "unknown");
    assert_eq!(result["hotspot_selection"]["weight_unit"], "bytes");
    assert_eq!(result["hotspot_selection"]["after_available"], false);
}

#[tokio::test]
async fn a_hotspot_below_the_other_sides_rank_limit_is_not_declared_removed() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    for run in ["before", "after"] {
        store.save_run(&recording(run)).unwrap();
        let mut rows = (0..1000)
            .map(|index| sample(run, &format!("filler-{index:04}"), "samples", 1.))
            .collect::<Vec<_>>();
        rows.push(sample(
            run,
            "rank-crossing",
            "samples",
            if run == "before" { 2. } else { 0.5 },
        ));
        store.accept(run, "input", &rows).unwrap();
    }
    let result = Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    let crossing = hotspot(&result, "rank-crossing");
    assert_eq!(crossing["before"], 2.);
    assert_eq!(crossing["after"], Value::Null);
    assert_eq!(crossing["delta"], Value::Null);
    assert_eq!(crossing["presence"], "unknown");
    assert_eq!(result["hotspot_selection"]["before_total_functions"], 1001);
    assert_eq!(result["hotspot_selection"]["after_truncated"], true);
}

#[tokio::test]
async fn observed_absence_and_new_hotspots_are_distinct_from_zero_self_weight() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    for run in ["before", "after"] {
        store.save_run(&recording(run)).unwrap();
    }
    let mut non_leaf = sample("before", "zero", "samples", 1.);
    non_leaf["stack"] = json!([{"function":"zero"},{"function":"leaf"}]);
    store
        .accept(
            "before",
            "input",
            &[sample("before", "removed", "samples", 3.), non_leaf],
        )
        .unwrap();
    store
        .accept(
            "after",
            "input",
            &[
                sample("after", "new", "samples", 4.),
                sample("after", "zero", "samples", 1.),
            ],
        )
        .unwrap();
    let result = Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    assert_eq!(hotspot(&result, "removed")["after"], 0.);
    assert_eq!(
        hotspot(&result, "removed")["presence"],
        "before_only_observed"
    );
    assert_eq!(hotspot(&result, "new")["before"], 0.);
    assert_eq!(hotspot(&result, "new")["presence"], "after_only_observed");
    assert_eq!(hotspot(&result, "zero")["presence"], "both_observed");
}

#[tokio::test]
async fn explicit_unit_selection_does_not_compare_unrelated_extra_units() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    for run in ["before", "after"] {
        store.save_run(&recording(run)).unwrap();
    }
    store
        .accept(
            "before",
            "input",
            &[
                sample("before", "allocated", "bytes", 500.),
                sample("before", "cpu", "samples", 3.),
            ],
        )
        .unwrap();
    store
        .accept("after", "input", &[sample("after", "cpu", "samples", 2.)])
        .unwrap();
    let result = Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after","weight_unit":"samples"}),
        )
        .await
        .unwrap();
    assert_eq!(result["comparable"], true, "{result}");
    assert_eq!(result["hotspot_selection"]["weight_unit"], "samples");
    assert_eq!(hotspot(&result, "cpu")["before"], 3.);
    assert_eq!(hotspot(&result, "cpu")["after"], 2.);
    assert!(
        result["hotspots"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["function"] != "allocated")
    );
}

#[tokio::test]
async fn two_unknown_supervised_scopes_do_not_establish_measurement_equivalence() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    for run in ["before", "after"] {
        let mut run = recording(run);
        run.source
            .as_object_mut()
            .unwrap()
            .remove("supervised_execution_scope");
        store.save_run(&run).unwrap();
    }
    let result = Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    let elapsed = metric(&result, "supervised.elapsed");
    assert_eq!(elapsed["comparable"], false);
    assert_eq!(elapsed["delta"], Value::Null);
    assert_eq!(elapsed["measurement_quality"], "unknown_scope");
    assert_eq!(elapsed["values_available"], true);
    assert_eq!(elapsed["measurement_comparable"], false);
    assert_eq!(elapsed["workload_comparable"], false);
}

#[tokio::test]
async fn a_one_sided_resource_measurement_is_retained_as_unavailable() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    for run in ["before", "after"] {
        store.save_run(&recording(run)).unwrap();
    }
    let point = |id, time, value| json!({"schema_version":1,"kind":"metric","id":id,"run_id":"before","producer_id":"control","pid":1,"tid":"main","timestamp_us":time,"basis":"observed","name":"process.rss","value":value,"unit":"bytes","scope":"process","attributes":{"process_lifetime_id":"one"}});
    store
        .accept(
            "before",
            "input",
            &[point("first", 10, 5.), point("second", 20, 8.)],
        )
        .unwrap();
    let result = Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    let rss = metric(&result, "process.rss.max_process");
    assert_eq!(rss["before"], 8.);
    assert_eq!(rss["after"], Value::Null);
    assert_eq!(rss["delta"], Value::Null);
    assert_eq!(rss["comparable"], false);
    assert_eq!(rss["after_observations"], 0);
}

#[tokio::test]
async fn imported_ingestion_timestamps_are_not_recorded_execution_durations() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    for id in ["before", "after"] {
        let mut run = recording(id);
        run.runtime = "imported".into();
        store.save_run(&run).unwrap();
    }
    let result = Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    let elapsed = metric(&result, "recording.elapsed");
    assert_eq!(elapsed["before"], Value::Null);
    assert_eq!(elapsed["after"], Value::Null);
    assert_eq!(elapsed["delta"], Value::Null);
    assert_eq!(elapsed["comparable"], false);
    assert!(
        elapsed["diagnostic"]
            .as_str()
            .unwrap()
            .contains("ingestion")
    );
}

#[tokio::test]
async fn a_missing_recording_completion_time_is_not_a_zero_duration() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let mut before = recording("before");
    before.finished_at_us = None;
    store.save_run(&before).unwrap();
    store.save_run(&recording("after")).unwrap();
    let result = Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    let elapsed = metric(&result, "recording.elapsed");
    assert_eq!(elapsed["before"], Value::Null);
    assert_eq!(elapsed["after"], 0.02);
    assert_eq!(elapsed["delta"], Value::Null);
    assert_eq!(elapsed["comparable"], false);
}

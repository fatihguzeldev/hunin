use hunin::{
    model::runs::Run,
    operations::{Operations, catalog},
    storage::Store,
};
use serde_json::{Value, json};

fn fixture() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store
        .save_run(&Run {
            id: "recording".into(),
            name: "inspection oracle".into(),
            status: "completed".into(),
            command: vec![],
            cwd: "/fixture".into(),
            runtime: "fixture".into(),
            started_at_us: 0,
            finished_at_us: Some(50),
            exit_code: Some(0),
            diagnostics: vec![],
            source: json!({}),
            correctness: json!({}),
        })
        .unwrap();
    let frame = |name: &str| json!({"function":name,"file":"work.ts","line":3,"module":"application","code_id":"build-v1"});
    let mut rows = vec![];
    for (id, time, weight, unit, names, aligned) in [
        ("s1", 10, 2, "samples", vec!["root", "hot", "hot"], true),
        ("s2", 20, 3, "samples", vec!["root", "cold"], true),
        ("s3", 30, 5, "samples", vec!["root", "hot"], true),
        ("bytes", 40, 7, "bytes", vec!["root", "bytes"], true),
        ("unmapped", 25, 101, "samples", vec!["unmapped"], false),
    ] {
        rows.push(json!({"schema_version":1,"kind":"sample","id":id,"run_id":"recording","producer_id":"profiler","pid":1,"tid":"main","timestamp_us":time,"basis":"sampled","weight":weight,"weight_unit":unit,"stack":names.into_iter().map(frame).collect::<Vec<_>>(),"attributes":{"clock":if aligned {"unix_epoch"}else{"thread_cpu"},"clock_alignment":if aligned{"direct_epoch"}else{"unmapped"}}}));
    }
    store.accept("recording", "oracle", &rows).unwrap();
    (root, store)
}

async fn inspect(operations: &Operations, params: Value) -> Value {
    operations.call("profiles.inspect", params).await.unwrap()
}

#[tokio::test]
async fn window_profile_coordinates_bins_frames_recursion_and_global_locate_extent() {
    let (_root, store) = fixture();
    let identity = store.records_snapshot("recording").unwrap().result_identity;
    let operations = Operations::new(store);
    let full = inspect(
        &operations,
        json!({"run_id":"recording","weight_unit":"samples","bins":2}),
    )
    .await;
    let hot = full["functions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["function"] == "hot")
        .unwrap();
    let selected=inspect(&operations,json!({"run_id":"recording","weight_unit":"samples","start_us":10,"end_us":20,"bins":2,"frame_id":hot["id"],"function_limit":1})).await;
    assert_eq!(selected["result_identity"], identity);
    assert_eq!(selected["bounds"], json!({"start_us":10,"end_us":30}));
    assert_eq!(selected["window"], json!({"start_us":10,"end_us":20}));
    assert_eq!(selected["sample_count"], 2);
    assert_eq!(selected["total_sample_count"], 3);
    assert_eq!(selected["total_weight"], 5.0);
    assert_eq!(selected["capture_weight"], 10.0);
    assert_eq!(selected["unaligned_count"], 1);
    assert_eq!(selected["overview"][0]["weight"], 2.0);
    assert_eq!(selected["overview"][1]["weight"], 8.0);
    assert_eq!(selected["overview"][0]["selected_weight"], 2.0);
    assert_eq!(selected["overview"][1]["selected_weight"], 5.0);
    assert_eq!(selected["activity"][0]["weight"], 2.0);
    assert_eq!(selected["activity"][1]["weight"], 3.0);
    assert_eq!(selected["selected"]["self_weight"], 2.0);
    assert_eq!(selected["selected"]["inclusive_weight"], 2.0);
    assert_eq!(selected["selected"]["sample_count"], 1);
    assert_eq!(selected["selected"]["observed_start_us"], 10);
    assert_eq!(selected["selected"]["observed_end_us"], 30);
    assert_eq!(selected["selected"]["evidence_ids"], json!(["s1"]));
    assert_eq!(selected["total_functions"], 3);
    assert_eq!(selected["functions_truncated"], true);
    let root = &selected["roots"][0];
    assert_eq!(root["total_weight"], 5.0);
    assert_ne!(root["id"], root["frame_id"]);
    let operation = catalog()
        .iter()
        .find(|v| v["name"] == "profiles.inspect")
        .unwrap();
    jsonschema::validator_for(&operation["outputSchema"])
        .unwrap()
        .validate(&selected)
        .unwrap();
}

#[tokio::test]
async fn selected_function_survives_an_empty_window_without_fabricating_measurements() {
    let (_root, store) = fixture();
    let operations = Operations::new(store);
    let full = inspect(
        &operations,
        json!({"run_id":"recording","weight_unit":"samples"}),
    )
    .await;
    let hot = full["functions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["function"] == "hot")
        .unwrap();
    let empty=inspect(&operations,json!({"run_id":"recording","weight_unit":"samples","start_us":21,"end_us":29,"frame_id":hot["id"]})).await;
    assert_eq!(empty["sample_count"], 0);
    assert_eq!(empty["total_weight"], 0.0);
    assert_eq!(empty["selected"]["self_weight"], 0.0);
    assert_eq!(empty["selected"]["inclusive_weight"], 0.0);
    assert_eq!(empty["selected"]["observed_start_us"], 10);
    assert_eq!(empty["selected"]["observed_end_us"], 30);
    assert_eq!(empty["selected"]["evidence_count"], 0);
    assert_eq!(empty["selected"]["evidence_ids"], json!([]));
    let missing = inspect(
        &operations,
        json!({"run_id":"recording","weight_unit":"microseconds"}),
    )
    .await;
    assert!(missing["bounds"].is_null());
    assert!(missing["total_weight"].is_null());
    assert!(missing["capture_weight"].is_null());
    let bounded = inspect(
        &operations,
        json!({"run_id":"recording","node_limit":1,"depth_limit":1}),
    )
    .await;
    assert_eq!(bounded["tree_truncated"], true);
}

#[tokio::test]
async fn inspection_rejects_ambiguous_ranges_and_reports_untimed_units() {
    let (_root, store) = fixture();
    let row = json!({"schema_version":1,"kind":"sample","id":"cpu-only","run_id":"recording","producer_id":"profiler","pid":1,"tid":"main","timestamp_us":42,"basis":"sampled","weight":11,"weight_unit":"count","stack":[{"function":"cpu-only"}],"attributes":{"clock":"thread_cpu","clock_alignment":"unmapped"}});
    store.accept("recording", "cpu-only", &[row]).unwrap();
    let operations = Operations::new(store);
    for params in [
        json!({"run_id":"recording","start_us":10}),
        json!({"run_id":"recording","start_us":30,"end_us":20}),
        json!({"run_id":"recording","bins":241}),
    ] {
        let error = operations
            .call("profiles.inspect", params)
            .await
            .unwrap_err();
        assert_eq!(
            hunin::operations::describe_failure("profiles.inspect", &error)["code"],
            "invalid_parameters"
        );
    }
    let result = inspect(
        &operations,
        json!({"run_id":"recording","weight_unit":"count"}),
    )
    .await;
    assert_eq!(result["unaligned_count"], 1);
    assert_eq!(result["sample_count"], 0);
    assert!(result["bounds"].is_null());
    assert!(result["total_weight"].is_null());
    assert!(result["capture_weight"].is_null());
    let one = inspect(
        &operations,
        json!({"run_id":"recording","weight_unit":"bytes"}),
    )
    .await;
    assert_eq!(one["bounds"], json!({"start_us":40,"end_us":41}));
    assert_eq!(one["total_weight"], 7.0);
    assert_eq!(one["sample_count"], 1);
}

#[test]
fn inspection_uses_existing_read_and_aggregation_work_limits() {
    use hunin::query::{self, QueryLimits, RecordingQuery, WorkBudgetExceeded, WorkDimension};
    use hunin::storage::{ReadBudgetExceeded, ReadDimension};
    let (_root, store) = fixture();
    let params = json!({"weight_unit":"samples"});
    let mut limits = QueryLimits::default();
    limits.read.maximum_rows = 1;
    let error = query::read_recording(
        &store,
        "recording",
        RecordingQuery::Inspect(&params),
        limits,
        || Ok(()),
    )
    .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<ReadBudgetExceeded>()
            .unwrap()
            .dimension,
        ReadDimension::Rows
    );
    let limits = QueryLimits {
        maximum_frame_visits: 1,
        ..Default::default()
    };
    let error = query::read_recording(
        &store,
        "recording",
        RecordingQuery::Inspect(&params),
        limits,
        || Ok(()),
    )
    .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<WorkBudgetExceeded>()
            .unwrap()
            .dimension,
        WorkDimension::FrameVisits
    );
}

#[tokio::test]
async fn overflowing_weights_remain_unknown_across_subsequent_samples_and_views() {
    let (_root, store) = fixture();
    let rows=(0..3).map(|index|json!({"schema_version":1,"kind":"sample","id":format!("huge-{index}"),"run_id":"recording","producer_id":"profiler","pid":1,"tid":"main","timestamp_us":100+index,"basis":"sampled","weight":1e308,"weight_unit":"microseconds","stack":[{"function":"root"},{"function":"huge"}],"attributes":{"clock":"unix_epoch","clock_alignment":"direct_epoch"}})).collect::<Vec<_>>();
    store.accept("recording", "huge", &rows).unwrap();
    let operations = Operations::new(store);
    let result = inspect(
        &operations,
        json!({"run_id":"recording","weight_unit":"microseconds","bins":1}),
    )
    .await;
    assert!(result["total_weight"].is_null());
    assert!(result["capture_weight"].is_null());
    assert!(result["overview"][0]["weight"].is_null());
    let huge = result["functions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["function"] == "huge")
        .unwrap();
    assert!(huge["self_weight"].is_null());
    assert!(huge["inclusive_weight"].is_null());
    let selected = inspect(
        &operations,
        json!({"run_id":"recording","weight_unit":"microseconds","bins":1,"frame_id":huge["id"]}),
    )
    .await;
    assert!(selected["overview"][0]["selected_weight"].is_null());
    assert!(selected["selected"]["self_weight"].is_null());
    assert!(selected["selected"]["callers"][0]["weight"].is_null());
}

#[tokio::test]
async fn integer_bin_edges_match_assigned_samples_when_duration_is_not_divisible() {
    let (_root, store) = fixture();
    let times = [0, 3, 4, 6, 7, 10];
    let records = times.into_iter().enumerate().map(|(index,timestamp)|json!({"schema_version":1,"kind":"sample","id":format!("edge-{index}"),"run_id":"recording","producer_id":"profiler","pid":1,"tid":"main","timestamp_us":timestamp,"basis":"sampled","weight":1u64<<index,"weight_unit":"count","stack":[{"function":"edge"}],"attributes":{"clock":"unix_epoch","clock_alignment":"direct_epoch"}})).collect::<Vec<_>>();
    store.accept("recording", "bin-edges", &records).unwrap();
    let data = inspect(
        &Operations::new(store),
        json!({"run_id":"recording","weight_unit":"count","start_us":0,"end_us":10,"bins":3}),
    )
    .await;
    let bins = data["activity"].as_array().unwrap();
    assert_eq!(
        bins.iter()
            .map(|bin| (
                bin["start_us"].as_u64().unwrap(),
                bin["end_us"].as_u64().unwrap()
            ))
            .collect::<Vec<_>>(),
        vec![(0, 4), (4, 7), (7, 10)]
    );
    assert_eq!(
        bins.iter()
            .map(|bin| bin["weight"].as_f64().unwrap())
            .collect::<Vec<_>>(),
        vec![3.0, 12.0, 48.0]
    );
    assert_eq!(
        bins.iter()
            .map(|bin| bin["sample_count"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![2, 2, 2]
    );
    assert_eq!(data["sample_count"], 6);
    assert_eq!(data["total_weight"], 63.0);
}

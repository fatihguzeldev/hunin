use hunin::{
    model::runs::Run,
    query::{self, QueryLimits, RecordingQuery, WorkBudgetExceeded, WorkDimension},
    storage::{ReadBudgetExceeded, ReadDimension, Store},
};
use serde_json::{Value, json};

fn fixture() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store
        .save_run(&Run {
            id: "recording".into(),
            name: "streaming oracle".into(),
            status: "completed".into(),
            command: vec![],
            cwd: "/fixture".into(),
            runtime: "fixture".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: Some(0),
            diagnostics: vec![],
            source: json!({}),
            correctness: json!({}),
        })
        .unwrap();
    let mut rows = vec![];
    for (index, kind, timestamp) in [
        (0, "sample", 10),
        (1, "span", 100),
        (2, "sample", 20),
        (3, "event", 120),
        (4, "sample", 30),
        (5, "event", 150),
    ] {
        let mut row = json!({"schema_version":1,"kind":kind,"id":format!("row-{index}"),"run_id":"recording","producer_id":format!("producer-{}",index%2),"pid":1+index%2,"tid":"thread","timestamp_us":timestamp,"basis":"observed","attributes":{"extension":{"preserved":[1,null,{"nested":true}]}}});
        if kind == "sample" {
            row["basis"] = json!("sampled");
            row["weight"] = json!(index + 1);
            row["weight_unit"] = json!(if index == 4 { "bytes" } else { "samples" });
            row["stack"] = json!([{"function":"root","file":"work.ts","line":1,"code_id":"v1"},{"function":"hot","file":"work.ts","line":index+2,"code_id":if index==2 {"v2"}else{"v1"}},{"function":"root","file":"work.ts","line":1,"code_id":"v1"}]);
        }
        if kind == "span" {
            row["duration_us"] = json!(100);
            row["name"] = json!("work");
        }
        if kind == "event" {
            row["name"] = json!("event");
        }
        if index == 0 || index == 5 {
            row["attributes"]["clock_alignment"] = json!("unmapped");
            row["attributes"]["clock"] = json!(if index == 0 {
                "thread_cpu"
            } else {
                "process_cpu"
            });
        }
        rows.push(row);
    }
    for index in 0..6 {
        rows.push(json!({"schema_version":1,"kind":"metric","id":format!("metric-{index}"),"run_id":"recording","producer_id":"resources","pid":1,"tid":"process","timestamp_us":100+index,"basis":"observed","name":"process.rss","value":index+1,"unit":"bytes","scope":"process","attributes":{"process_lifetime_id":if index<3 {"first"}else{"second"},"clock_alignment":if index==5 {"unmapped"}else{"aligned"},"clock":if index==5 {"process_cpu"}else{"unix_epoch"}}}));
    }
    store.accept("recording", "fixture", &rows).unwrap();
    (root, store)
}

fn check(store: &Store, query: RecordingQuery<'_>, expected: Value) {
    let oracle = store.records_snapshot("recording").unwrap();
    let result =
        query::read_recording(store, "recording", query, QueryLimits::default(), || Ok(()))
            .unwrap();
    assert_eq!(result.value, expected);
    assert_eq!(result.result_identity, oracle.result_identity);
}

#[test]
fn streamed_queries_preserve_slice_outputs_clocks_navigation_and_selected_identity() {
    let (_root, store) = fixture();
    let rows = store.records("recording").unwrap();
    check(&store, RecordingQuery::Summary, query::summary(&rows));
    for unit in ["", "samples", "bytes", "count"] {
        check(
            &store,
            RecordingQuery::Hotspots { unit, limit: 2 },
            query::hotspots(&rows, unit, 2),
        );
    }
    let first = query::tree_page(&rows, &json!({"limit":1}));
    for params in [
        json!({}),
        json!({"limit":1}),
        json!({"cursor":"1","depth_limit":1}),
        json!({"path":[first["roots"][0]["id"]],"limit":2}),
        json!({"weight_unit":"bytes","depth_limit":2}),
    ] {
        check(
            &store,
            RecordingQuery::Tree(&params),
            query::tree_page(&rows, &params),
        );
    }
    for params in [
        json!({}),
        json!({"limit":1}),
        json!({"cursor":"2","limit":2}),
        json!({"kind":"sample","alignment":"all"}),
        json!({"kind":"metric","pid":1,"start_us":102,"end_us":103,"limit":1}),
        json!({"kind":"span","start_us":150,"end_us":160}),
        json!({"pid":2,"alignment":"unmapped","start_us":1000}),
        json!({"alignment":"all","cursor":"1","limit":2,"start_us":102,"end_us":103}),
        json!({"alignment":"unmapped","cursor":"1","limit":1,"end_us":0}),
        json!({"kind":"relation"}),
    ] {
        check(
            &store,
            RecordingQuery::Timeline(&params),
            query::timeline(&rows, &params),
        );
    }
    for params in [
        json!({}),
        json!({"limit":1,"point_limit":1}),
        json!({"cursor":"2","point_limit":2}),
        json!({"start_us":101,"end_us":103,"point_limit":1}),
        json!({"cursor":"1000","start_us":1000}),
    ] {
        check(
            &store,
            RecordingQuery::Resources(&params),
            query::resources_page(&rows, &params),
        );
    }
}

#[test]
fn historical_large_pids_and_null_sample_positions_keep_existing_json_semantics() {
    let (_root, store) = fixture();
    let mut rows = store.records("recording").unwrap();
    let row = rows.iter_mut().find(|row| row["id"] == "row-2").unwrap();
    row["pid"] = serde_json::from_str("184467440737095516160").unwrap();
    row["stack"][0]["line"] = Value::Null;
    row["stack"][0]["file"] = Value::Null;
    let connection = rusqlite::Connection::open(store.root().join("hunin.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE observations SET record=?1 WHERE id='row-2'",
            [serde_json::to_string(row).unwrap()],
        )
        .unwrap();
    let rows = store.records("recording").unwrap();
    check(&store, RecordingQuery::Summary, query::summary(&rows));
    check(
        &store,
        RecordingQuery::Hotspots {
            unit: "samples",
            limit: 100,
        },
        query::hotspots(&rows, "samples", 100),
    );
    for params in [
        json!({"alignment":"all"}),
        json!({"pid":1}),
        json!({"pid":18446744073709551615_u64}),
    ] {
        check(
            &store,
            RecordingQuery::Timeline(&params),
            query::timeline(&rows, &params),
        );
    }
}

#[test]
fn frame_and_aggregate_limits_fail_without_publishing_partial_query_results() {
    let (_root, store) = fixture();
    for query in [
        RecordingQuery::Hotspots {
            unit: "samples",
            limit: 10,
        },
        RecordingQuery::Tree(&Value::Null),
    ] {
        let error = query::read_recording(
            &store,
            "recording",
            query,
            QueryLimits {
                maximum_frame_visits: 1,
                ..Default::default()
            },
            || Ok(()),
        )
        .unwrap_err();
        let error = error.downcast_ref::<WorkBudgetExceeded>().unwrap();
        assert_eq!(error.dimension, WorkDimension::FrameVisits);
        assert_eq!(error.observed, 2);
    }
    for query in [
        RecordingQuery::Summary,
        RecordingQuery::Hotspots {
            unit: "samples",
            limit: 10,
        },
        RecordingQuery::Tree(&Value::Null),
        RecordingQuery::Resources(&Value::Null),
        RecordingQuery::Timeline(&json!({"alignment":"all"})),
    ] {
        let error = query::read_recording(
            &store,
            "recording",
            query,
            QueryLimits {
                maximum_aggregate_entries: 0,
                ..Default::default()
            },
            || Ok(()),
        )
        .unwrap_err();
        let error = error.downcast_ref::<WorkBudgetExceeded>().unwrap();
        assert_eq!(error.dimension, WorkDimension::AggregateEntries);
        assert_eq!(error.observed, 1);
    }
}

#[test]
fn resource_and_timeline_kind_selectors_work_when_broad_input_exceeds_its_budget() {
    let (_root, store) = fixture();
    let mut sample = store
        .records("recording")
        .unwrap()
        .into_iter()
        .find(|row| row["id"] == "row-0")
        .unwrap();
    sample["attributes"]["large_extension"] = json!("x".repeat(10000));
    let connection = rusqlite::Connection::open(store.root().join("hunin.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE observations SET record=?1 WHERE id='row-0'",
            [serde_json::to_string(&sample).unwrap()],
        )
        .unwrap();
    let mut limits = QueryLimits::default();
    limits.read.maximum_record_bytes = 2000;
    let error = query::read_recording(
        &store,
        "recording",
        RecordingQuery::Summary,
        limits.clone(),
        || Ok(()),
    )
    .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<ReadBudgetExceeded>()
            .unwrap()
            .dimension,
        ReadDimension::RecordBytes
    );
    let resources = query::read_recording(
        &store,
        "recording",
        RecordingQuery::Resources(&Value::Null),
        limits.clone(),
        || Ok(()),
    )
    .unwrap();
    assert_eq!(resources.usage.rows, 6);
    let timeline = query::read_recording(
        &store,
        "recording",
        RecordingQuery::Timeline(&json!({"kind":"metric"})),
        limits,
        || Ok(()),
    )
    .unwrap();
    assert_eq!(timeline.usage.rows, 6);
    assert_eq!(
        timeline.value["bounds"],
        json!({"start_us":100,"end_us":104})
    );
    assert_eq!(timeline.value["unaligned_count"], 1);
}

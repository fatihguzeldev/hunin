use hunin::{
    model::runs::Run,
    query::{self, QueryLimits, RecordingQuery},
    storage::Store,
};
use serde_json::{Value, json};
fn setup() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store
        .save_run(&Run {
            id: "clocks".into(),
            name: "clock contract".into(),
            status: "completed".into(),
            command: vec![],
            cwd: "/fixture".into(),
            runtime: "imported".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: None,
            diagnostics: vec![],
            source: json!({}),
            correctness: json!({}),
        })
        .unwrap();
    (root, store)
}
fn metric(id: &str, attributes: Value) -> Value {
    json!({"schema_version":1,"kind":"metric","id":id,"run_id":"clocks","producer_id":id,"pid":1,"tid":"process","timestamp_us":100,"basis":"observed","name":"rss","value":7,"unit":"bytes","scope":"process","attributes":attributes})
}
#[test]
fn missing_unknown_and_opaque_clocks_remain_unaligned_across_independent_producers() {
    let (_root, store) = setup();
    let rows = vec![
        metric("missing", json!({})),
        metric(
            "unknown",
            json!({"clock":"unknown","clock_alignment":"unknown"}),
        ),
        metric(
            "future",
            json!({"clock":"unix_epoch","clock_alignment":"future_method"}),
        ),
        metric(
            "opaque",
            json!({"clock":{"domain":"opaque"},"clock_alignment":"unmapped"}),
        ),
        metric("null", json!({"clock":null})),
        metric(
            "epoch",
            json!({"clock":"unix_epoch","clock_alignment":"direct_epoch","clock_source":"fixture epoch sample"}),
        ),
    ];
    store.accept("clocks", "source", &rows).unwrap();
    let params = json!({"alignment":"all","start_us":99,"end_us":101});
    let direct = query::timeline(&rows, &params);
    let streamed = query::read_recording(
        &store,
        "clocks",
        RecordingQuery::Timeline(&params),
        QueryLimits::default(),
        || Ok(()),
    )
    .unwrap()
    .value;
    for result in [direct, streamed] {
        assert_eq!(result["records"].as_array().unwrap().len(), 1);
        assert_eq!(result["unaligned_records"].as_array().unwrap().len(), 5);
        assert_eq!(result["unaligned_count"], 5);
    }
    let result = query::resources_page(&rows, &json!({"start_us":999,"end_us":1000}));
    assert_eq!(result["total_points"], 5);
    assert_eq!(result["unaligned_point_count"], 5);
    assert!(
        result["series"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["clock_alignment"] == "unmapped")
    );
    assert_eq!(store.records("clocks").unwrap(), rows);
}
#[test]
fn declared_epoch_mapping_rejects_contradictions_and_bad_types_atomically() {
    let (_root, store) = setup();
    let before = metric(
        "before",
        json!({"clock":"unix_epoch","clock_alignment":"direct_epoch"}),
    );
    store
        .accept_import("clocks", "source", "first", std::slice::from_ref(&before))
        .unwrap();
    for attrs in [
        json!({"clock":"process_cpu","clock_alignment":"direct_epoch"}),
        json!({"clock_alignment":"runtime_anchor"}),
        json!({"clock":"unix_epoch","clock_alignment":7}),
    ] {
        let error = store
            .accept_import("clocks", "source", "bad", &[metric("bad", attrs)])
            .unwrap_err();
        assert!(error.to_string().contains("clock"), "{error}");
        assert_eq!(store.records("clocks").unwrap(), vec![before.clone()]);
    }
}
#[test]
fn concrete_legacy_epoch_sources_retain_provenance_and_malformed_history_stays_unknown() {
    let (root, store) = setup();
    let legacy = metric("legacy", json!({"clock":"time.time_ns/perf_counter_ns"}));
    store
        .accept("clocks", "legacy", std::slice::from_ref(&legacy))
        .unwrap();
    let resources = query::resources(&[legacy.clone()]);
    assert_eq!(resources["series"][0]["clock_alignment"], "aligned");
    assert_eq!(
        resources["series"][0]["clock"],
        legacy["attributes"]["clock"]
    );
    assert_eq!(
        resources["series"][0]["time_mapping"],
        "legacy_epoch_source"
    );
    let mut old = legacy;
    old["attributes"] = json!({"clock":"process_cpu","clock_alignment":true});
    let raw = serde_json::to_string(&old).unwrap();
    let c = rusqlite::Connection::open(root.path().join("hunin.sqlite")).unwrap();
    c.execute("UPDATE observations SET record=?1", [&raw])
        .unwrap();
    let retained = store.records("clocks").unwrap();
    assert_eq!(query::timeline(&retained, &json!({}))["unaligned_count"], 1);
    assert_eq!(
        query::resources(&retained)["series"][0]["clock_alignment"],
        "unmapped"
    );
    let bytes: String = c
        .query_row("SELECT record FROM observations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(bytes, raw);
}

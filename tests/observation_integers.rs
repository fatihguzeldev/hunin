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
            id: "numbers".into(),
            name: "numeric boundary".into(),
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
fn span() -> Value {
    json!({"schema_version":1,"kind":"span","id":"span","run_id":"numbers","producer_id":"fixture","pid":2.0,"tid":"main","timestamp_us":100.0,"duration_us":20.0,"basis":"explicit","name":"work","attributes":{"clock":"unix_epoch","clock_alignment":"aligned","opaque":{"integer_looking":2.0}}})
}
fn assert_window(store: &Store) {
    let params = json!({"start_us":110,"end_us":115,"pid":2});
    let direct = query::timeline(&store.records("numbers").unwrap(), &params);
    let streamed = query::read_recording(
        store,
        "numbers",
        RecordingQuery::Timeline(&params),
        QueryLimits::default(),
        || Ok(()),
    )
    .unwrap()
    .value;
    for result in [direct, streamed] {
        assert_eq!(result["total_count"], 1);
        assert_eq!(result["bounds"], json!({"start_us":100,"end_us":120}));
    }
}
#[test]
fn integral_json_numbers_publish_query_and_replay_without_changing_retained_bytes() {
    let (root, store) = setup();
    let record = span();
    assert!(
        store
            .accept("numbers", "input", std::slice::from_ref(&record))
            .unwrap()
    );
    assert_window(&store);
    assert_eq!(store.records("numbers").unwrap(), vec![record.clone()]);
    assert!(
        !store
            .accept("numbers", "input", std::slice::from_ref(&record))
            .unwrap()
    );
    drop(store);
    let store = Store::open(root.path()).unwrap();
    assert_window(&store);
    let connection = rusqlite::Connection::open(root.path().join("hunin.sqlite")).unwrap();
    let (time, raw): (u64, String) = connection
        .query_row("SELECT timestamp_us,record FROM observations", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(time, 100);
    assert_eq!(raw, serde_json::to_string(&record).unwrap());
}
#[test]
fn historical_integral_float_records_keep_time_and_pid_semantics_without_rewriting_history() {
    let (root, store) = setup();
    let mut integer = span();
    integer["timestamp_us"] = json!(100);
    integer["duration_us"] = json!(20);
    integer["pid"] = json!(2);
    store.accept("numbers", "input", &[integer]).unwrap();
    let raw = serde_json::to_string(&span()).unwrap();
    let connection = rusqlite::Connection::open(root.path().join("hunin.sqlite")).unwrap();
    connection
        .execute("UPDATE observations SET record=?1", [&raw])
        .unwrap();
    assert_window(&store);
    let retained: String = connection
        .query_row("SELECT record FROM observations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(retained, raw);
}

#[test]
fn unbounded_schema_integer_metadata_remains_accepted_without_lossy_conversion() {
    let (root, store) = setup();
    let mut record = span();
    let beyond_u64: Value = serde_json::from_str("18446744073709551616.0").unwrap();
    record["pid"] = beyond_u64.clone();
    record["stack"] = json!([{
        "function": "opaque_location",
        "line": beyond_u64,
        "column": beyond_u64,
    }]);
    let raw = serde_json::to_string(&record).unwrap();
    assert!(
        store
            .accept("numbers", "input", std::slice::from_ref(&record))
            .unwrap()
    );
    assert_eq!(store.records("numbers").unwrap(), vec![record.clone()]);
    drop(store);
    let store = Store::open(root.path()).unwrap();
    assert_eq!(store.records("numbers").unwrap(), vec![record]);
    let connection = rusqlite::Connection::open(root.path().join("hunin.sqlite")).unwrap();
    let retained: String = connection
        .query_row("SELECT record FROM observations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(retained, raw);
}

#[test]
fn invalid_numeric_replacement_leaves_selected_observations_unchanged() {
    let (_root, store) = setup();
    let record = span();
    store
        .accept_import("numbers", "source", "first", std::slice::from_ref(&record))
        .unwrap();
    for (field, value) in [
        ("timestamp_us", json!(100.5)),
        ("duration_us", json!(-1)),
        ("timestamp_us", json!(9007199254740992u64)),
    ] {
        let mut bad = record.clone();
        bad["id"] = json!("bad");
        bad[field] = value;
        let error = store
            .accept_import("numbers", "source", "bad", &[bad])
            .unwrap_err();
        assert!(error.to_string().contains("observation"), "{error}");
        assert_eq!(store.records("numbers").unwrap(), vec![record.clone()]);
    }
}

#[tokio::test]
async fn equivalent_pid_encodings_share_resource_series_and_process_lifetime_cost() {
    let (_root, store) = setup();
    let rows: Vec<_> = [(100.0, 2.0, 5), (120.0, 2.0, 7)]
        .into_iter()
        .enumerate()
        .map(|(index, (time, pid, value))| {
            let mut record = span();
            record["id"] = json!(format!("point-{index}"));
            record["kind"] = json!("metric");
            record["name"] = json!("process.cpu_time");
            record.as_object_mut().unwrap().remove("duration_us");
            record["timestamp_us"] = json!(time);
            record["pid"] = if index == 0 { json!(2) } else { json!(pid) };
            record["value"] = json!(value);
            record["unit"] = json!("milliseconds");
            record["scope"] = json!("process");
            record["attributes"]["process_lifetime_id"] = json!("one");
            record["attributes"]["temporality"] = json!("cumulative");
            record
        })
        .collect();
    store.accept("numbers", "resources", &rows).unwrap();
    let resources = query::resources(&rows);
    assert_eq!(resources["series"].as_array().unwrap().len(), 1);
    let result = hunin::operations::Operations::new(store)
        .call(
            "comparisons.create",
            json!({"before":"numbers","after":"numbers"}),
        )
        .await
        .unwrap();
    let cpu = result["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "process.cpu_time.observed_total")
        .unwrap();
    assert_eq!(cpu["before"], 7.0);
    assert_eq!(cpu["before_process_lifetimes"], 1);
}

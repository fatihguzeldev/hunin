use hunin::{
    model::runs::Run,
    storage::{MetadataPageBudgetExceeded, MetadataPageDimension, Store},
};
use serde_json::{Value, json};

fn run(id: &str, started_at_us: u64) -> Run {
    Run {
        id: id.into(),
        name: id.into(),
        status: "completed".into(),
        command: vec![],
        cwd: ".".into(),
        runtime: "fixture".into(),
        started_at_us,
        finished_at_us: Some(started_at_us),
        exit_code: None,
        diagnostics: vec![],
        source: Value::Null,
        correctness: Value::Null,
    }
}

#[test]
fn metadata_pages_reject_large_selected_payloads_before_decoding_them() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let padding = "x".repeat(8 * 1024 * 1024);
    let mut large = run("large", 2);
    large.source = json!({"legacy_extension":padding});
    store.save_run(&large).unwrap();
    assert_row_budget(rejected(store.runs(1, 0)));
    store
        .save_document(
            "scenarios",
            "large",
            &json!({"created_at_us":2,"legacy_extension":padding}),
        )
        .unwrap();
    assert_row_budget(rejected(store.documents_page("scenarios", None, 1, 0)));

    store.save_run(&run("owner", 1)).unwrap();
    let mut artifact = store
        .add_artifact_bytes("owner", "raw.json", "raw", b"preserved")
        .unwrap();
    artifact.name = padding;
    let connection = rusqlite::Connection::open(root.path().join("hunin.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE artifacts SET metadata=?1 WHERE run_id='owner' AND id=?2",
            rusqlite::params![serde_json::to_string(&artifact).unwrap(), artifact.id],
        )
        .unwrap();
    assert_row_budget(rejected(store.artifacts_page("owner", 1, 0)));
}

fn rejected<T>(result: anyhow::Result<T>) -> anyhow::Error {
    // Avoid dumping megabytes of the unexpected accepted payload on failure.
    let Err(error) = result else {
        panic!("metadata page decoded an oversized selected payload");
    };
    error
}

fn assert_row_budget(error: anyhow::Error) {
    let error = error.downcast_ref::<MetadataPageBudgetExceeded>().unwrap();
    assert_eq!(error.dimension, MetadataPageDimension::RowBytes);
    assert_eq!(error.limit, 8 * 1024 * 1024);
    assert!(error.observed > error.limit);
}

#[test]
fn metadata_pages_preserve_creation_order_owner_filter_cursor_and_extensions() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    for (id, time) in [("b", 7), ("a", 7), ("older", 1)] {
        store.save_run(&run(id, time)).unwrap();
    }
    assert_eq!(
        store
            .runs(2, 1)
            .unwrap()
            .iter()
            .map(|run| run.id.as_str())
            .collect::<Vec<_>>(),
        ["b", "older"]
    );
    let values = [
        (
            "a",
            json!({"created_at_us":7,"run_id":"a","extension":{"clock":"legacy","pid":18446744073709551616.0}}),
        ),
        (
            "b",
            json!({"created_at_us":7,"run_id":"a","extension":[null, "é"]}),
        ),
        ("old", json!({"created_at_us":1,"run_id":"a"})),
        ("foreign", json!({"created_at_us":9,"run_id":"b"})),
    ];
    for (id, value) in &values {
        store.save_document("analysis-results", id, value).unwrap();
    }
    assert_eq!(
        store
            .documents_page("analysis-results", Some("a"), 2, 1)
            .unwrap(),
        vec![values[1].1.clone(), values[2].1.clone()]
    );
    assert_eq!(
        store
            .documents_page("analysis-results", None, 1, 0)
            .unwrap(),
        vec![values[3].1.clone()]
    );
    let fallback = json!({"extension":"no creation timestamp"});
    store
        .save_document("scenarios", "fallback", &fallback)
        .unwrap();
    assert_eq!(
        store.documents_page("scenarios", None, 1, 0).unwrap(),
        [fallback]
    );

    for name in ["first.raw", "second.raw", "third.raw"] {
        store.add_artifact_bytes("a", name, "raw", b"raw").unwrap();
    }
    let all = store.artifacts("a").unwrap();
    let page = store.artifacts_page("a", 2, 1).unwrap();
    assert_eq!(
        serde_json::to_value(page).unwrap(),
        serde_json::to_value(&all[1..]).unwrap()
    );
    assert!(store.artifacts_page("missing", 1, 0).is_err());
}

#[test]
fn skipped_payloads_are_not_decoded_but_the_requested_sentinel_is_budgeted() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let padding = "é".repeat(4 * 1024 * 1024);
    let mut oversized = run("older", 1);
    oversized.source = json!({"padding":padding});
    store.save_run(&oversized).unwrap();
    store.save_run(&run("newer", 2)).unwrap();
    assert_eq!(store.runs(1, 0).unwrap()[0].id, "newer");
    assert_row_budget(rejected(store.runs(2, 0)));
    let small = json!({"created_at_us":2,"legacy":[null,"preserved"]});
    store.save_document("scenarios", "newer", &small).unwrap();
    store
        .save_document(
            "scenarios",
            "older",
            &json!({"created_at_us":1,"padding":padding}),
        )
        .unwrap();
    assert_eq!(
        store.documents_page("scenarios", None, 1, 0).unwrap(),
        [small]
    );
    assert_row_budget(rejected(store.documents_page("scenarios", None, 2, 0)));
    let small = store
        .add_artifact_bytes("newer", "small.raw", "raw", b"raw")
        .unwrap();
    let mut large = small.clone();
    large.id = "zz-oversized".into();
    large.name = padding;
    let connection = rusqlite::Connection::open(root.path().join("hunin.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO artifacts(run_id,id,metadata,sha256) VALUES('newer',?1,?2,?3)",
            rusqlite::params![
                large.id,
                serde_json::to_string(&large).unwrap(),
                large.sha256
            ],
        )
        .unwrap();
    assert_eq!(store.artifacts_page("newer", 1, 0).unwrap()[0].id, small.id);
    assert_row_budget(rejected(store.artifacts_page("newer", 2, 0)));
}

#[test]
fn document_page_order_matches_the_previous_sql_for_legacy_timestamp_types() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    for (index, created) in [
        Value::Null,
        json!(true),
        json!(-2),
        json!(4.25),
        json!("legacy timestamp"),
        json!([7, null]),
        json!({"legacy":1}),
    ]
    .into_iter()
    .enumerate()
    {
        store
            .save_document(
                "scenarios",
                &format!("legacy-{index}"),
                &json!({"created_at_us":created,"extension":["é",null]}),
            )
            .unwrap();
    }
    let connection = rusqlite::Connection::open(root.path().join("hunin.sqlite")).unwrap();
    for (limit, offset) in [(3, 0), (3, 2), (9, 6), (0, 0)] {
        let mut old = connection.prepare("SELECT payload FROM documents WHERE namespace=?1 AND (?2 IS NULL OR owner_run_id=?2) ORDER BY COALESCE(json_extract(payload,'$.created_at_us'),updated_at_us) DESC,id LIMIT ?3 OFFSET ?4").unwrap();
        let expected = old
            .query_map(
                rusqlite::params!["scenarios", Option::<&str>::None, limit, offset],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
            .map(|text| serde_json::from_str::<Value>(&text.unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            store
                .documents_page("scenarios", None, limit, offset)
                .unwrap(),
            expected
        );
    }
}

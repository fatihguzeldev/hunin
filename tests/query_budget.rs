use hunin::{
    model::runs::Run,
    storage::{ReadBudgetExceeded, ReadDimension, ReadLimits, Store},
};
use serde_json::{Value, json};

fn exclusive_wal_writer(store: &Store) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(store.root().join("hunin.sqlite")).unwrap();
    let journal: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal, "wal");
    connection
        .execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE")
        .unwrap();
    connection
}

#[test]
fn readonly_snapshot_waits_for_a_short_wal_lock_without_replaying_visitors() {
    use std::{sync::mpsc, time::Duration};
    let (_root, store) = fixture();
    let writer = exclusive_wal_writer(&store);
    let reader = store.clone();
    let (started, ready) = mpsc::sync_channel(1);
    let read = std::thread::spawn(move || {
        let mut started = Some(started);
        reader.with_query_snapshot(
            "recording",
            ReadLimits::default(),
            move || {
                if let Some(started) = started.take() {
                    started.send(()).unwrap();
                }
                Ok(())
            },
            |port| {
                let mut rows = vec![];
                port.visit(&[], |text| {
                    rows.push(serde_json::from_str::<Value>(text)?);
                    Ok(())
                })?;
                Ok(rows)
            },
        )
    });
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    // A real WAL exclusive lock is present when the read starts. Release it
    // inside the reader's bounded SQLite wait, rather than retrying a query.
    std::thread::sleep(Duration::from_millis(50));
    drop(writer);
    let result = read
        .join()
        .unwrap()
        .expect("read rejected a short WAL lock");
    let expected = store.records_snapshot("recording").unwrap();
    assert_eq!(result.value, expected.records);
    assert_eq!(result.result_identity, expected.result_identity);
    assert_eq!(result.usage.rows, 3);
}

#[test]
fn readonly_snapshot_reports_busy_after_its_bounded_wait_before_consuming_rows() {
    let (_root, store) = fixture();
    let _writer = exclusive_wal_writer(&store);
    let mut consumed = false;
    let error = store
        .with_query_snapshot(
            "recording",
            ReadLimits::default(),
            || Ok(()),
            |_| {
                consumed = true;
                Ok(())
            },
        )
        .unwrap_err();
    let error = error.downcast_ref::<rusqlite::Error>().unwrap();
    assert_eq!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy)
    );
    assert!(!consumed);
}

#[test]
fn readonly_snapshot_keeps_the_interest_error_that_expires_during_a_busy_unit() {
    use std::{
        fmt,
        time::{Duration, Instant},
    };
    #[derive(Debug)]
    struct Abandoned;
    impl fmt::Display for Abandoned {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("read interest abandoned")
        }
    }
    impl std::error::Error for Abandoned {}
    let (_root, store) = fixture();
    let _writer = exclusive_wal_writer(&store);
    let began = Instant::now();
    let mut consumed = false;
    let error = store
        .with_query_snapshot(
            "recording",
            ReadLimits::default(),
            move || {
                if began.elapsed() >= Duration::from_millis(50) {
                    return Err(Abandoned.into());
                }
                Ok(())
            },
            |_| {
                consumed = true;
                Ok(())
            },
        )
        .unwrap_err();
    assert!(error.downcast_ref::<Abandoned>().is_some(), "{error:#}");
    assert!(!consumed);
}

fn fixture() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store
        .save_run(&Run {
            id: "recording".into(),
            name: "query budgets".into(),
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
    let records=(0..3).map(|index|json!({"schema_version":1,"kind":"event","id":format!("event-{index}"),"run_id":"recording","producer_id":"fixture","pid":1,"tid":"thread","timestamp_us":index,"name":"fixture","basis":"observed"})).collect::<Vec<_>>();
    store.accept("recording", "input", &records).unwrap();
    (root, store)
}

#[test]
fn row_limit_stops_before_the_sentinel_is_materialized() {
    let (_root, store) = fixture();
    let mut visited = 0;
    let result = store.with_query_snapshot(
        "recording",
        ReadLimits {
            maximum_rows: 2,
            ..Default::default()
        },
        || Ok(()),
        |port| {
            port.visit(&[], |text| {
                let _: Value = serde_json::from_str(text)?;
                visited += 1;
                Ok(())
            })
        },
    );
    let error = result.expect_err("query read ignored its row budget");
    let limit = error
        .downcast_ref::<ReadBudgetExceeded>()
        .expect("typed budget origin missing");
    assert_eq!(limit.dimension, ReadDimension::Rows);
    assert_eq!(limit.observed, 3);
    assert_eq!(visited, 2);
}

#[test]
fn pages_and_evidence_ids_select_before_payload_decoding_and_share_read_budgets() {
    let (_root, store) = fixture();
    let connection = rusqlite::Connection::open(store.root().join("hunin.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE observations SET record=?1 WHERE id='event-0'",
            ["invalid JSON".repeat(1000)],
        )
        .unwrap();
    let limits = ReadLimits {
        maximum_record_bytes: 1024,
        ..Default::default()
    };
    let page = store
        .with_query_snapshot(
            "recording",
            limits.clone(),
            || Ok(()),
            |port| {
                let mut ids = Vec::new();
                port.visit_page(&["event"], 1, 1, |text| {
                    ids.push(serde_json::from_str::<Value>(text)?["id"].clone());
                    Ok(())
                })?;
                Ok(ids)
            },
        )
        .unwrap();
    assert_eq!(page.value, vec![json!("event-1")]);
    assert_eq!(page.usage.rows, 1);
    let selected = store
        .with_query_snapshot(
            "recording",
            limits.clone(),
            || Ok(()),
            |port| {
                let mut ids = Vec::new();
                port.visit_ids(
                    &[
                        "missing".into(),
                        "event-2".into(),
                        "event-2".into(),
                        "event-1".into(),
                    ],
                    |text| {
                        ids.push(serde_json::from_str::<Value>(text)?["id"].clone());
                        Ok(())
                    },
                )?;
                Ok(ids)
            },
        )
        .unwrap();
    assert_eq!(selected.value, vec![json!("event-1"), json!("event-2")]);
    assert_eq!(selected.usage.rows, 2);
    let error = store
        .with_query_snapshot(
            "recording",
            limits,
            || Ok(()),
            |port| port.visit_ids(&["event-0".into()], |_| panic!("oversize payload decoded")),
        )
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<ReadBudgetExceeded>()
            .unwrap()
            .dimension,
        ReadDimension::RecordBytes
    );
    let error = store
        .with_query_snapshot(
            "recording",
            ReadLimits {
                maximum_rows: 1,
                ..Default::default()
            },
            || Ok(()),
            |port| {
                port.visit_page(&["event"], 1, 1, |_| Ok(()))?;
                port.visit_ids(&["event-2".into()], |_| {
                    panic!("second visit exceeded cumulative rows")
                })
            },
        )
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<ReadBudgetExceeded>()
            .unwrap()
            .dimension,
        ReadDimension::Rows
    );
}

#[test]
fn oversized_record_is_rejected_before_json_decoding() {
    let (_root, store) = fixture();
    let connection = rusqlite::Connection::open(store.root().join("hunin.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE observations SET record=?1 WHERE id='event-0'",
            ["invalid JSON".repeat(100)],
        )
        .unwrap();
    let mut visited = false;
    let result = store.with_query_snapshot(
        "recording",
        ReadLimits {
            maximum_record_bytes: 64,
            ..Default::default()
        },
        || Ok(()),
        |port| {
            port.visit(&[], |text| {
                visited = true;
                let _: Value = serde_json::from_str(text)?;
                Ok(())
            })
        },
    );
    let error = result.unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<ReadBudgetExceeded>()
            .unwrap()
            .dimension,
        ReadDimension::RecordBytes
    );
    assert!(!visited);
}

#[test]
fn byte_and_result_identity_limits_fail_at_their_actual_sentinels() {
    let (_root, store) = fixture();
    let sample = store
        .with_query_snapshot(
            "recording",
            ReadLimits::default(),
            || Ok(()),
            |port| port.visit(&[], |_| Ok(())),
        )
        .unwrap();
    let result = store.with_query_snapshot(
        "recording",
        ReadLimits {
            maximum_bytes: sample.usage.identity_bytes + 1,
            ..Default::default()
        },
        || Ok(()),
        |port| port.visit(&[], |_| Ok(())),
    );
    assert_eq!(
        result
            .err()
            .unwrap()
            .downcast_ref::<ReadBudgetExceeded>()
            .unwrap()
            .dimension,
        ReadDimension::Bytes
    );
    let result = store.with_query_snapshot(
        "recording",
        ReadLimits {
            maximum_result_sets: 0,
            ..Default::default()
        },
        || Ok(()),
        |_| Ok(()),
    );
    let error = result.unwrap_err();
    let error = error.downcast_ref::<ReadBudgetExceeded>().unwrap();
    assert_eq!(error.dimension, ReadDimension::ResultSets);
    assert_eq!(error.observed, 1);
}

#[test]
fn selected_identity_and_rows_share_a_snapshot_when_replacement_commits() {
    let (_root, store) = fixture();
    let expected = store.records_snapshot("recording").unwrap();
    let other = store.clone();
    let snapshot=store.with_query_snapshot("recording",ReadLimits::default(),||Ok(()),|port|{
        let mut records=vec![];
        port.visit(&[],|text|{
            if records.is_empty() {
                let new=json!({"schema_version":1,"kind":"event","id":"replacement","run_id":"recording","producer_id":"fixture","pid":1,"tid":"thread","timestamp_us":10,"name":"new","basis":"observed"});
                other.accept_import("recording","input","replacement",&[new])?;
            }
            records.push(serde_json::from_str::<Value>(text)?);
            Ok(())
        })?;
        Ok(records)
    }).unwrap();
    assert_eq!(snapshot.value, expected.records);
    assert_eq!(snapshot.result_identity, expected.result_identity);
    assert_ne!(
        store.records_snapshot("recording").unwrap().result_identity,
        snapshot.result_identity
    );
}

#[test]
fn inactive_history_vm_work_is_interrupted_before_the_first_selected_row() {
    let (_root, store) = fixture();
    let limits = ReadLimits {
        maximum_sqlite_steps: 2000,
        ..Default::default()
    };
    store
        .with_query_snapshot(
            "recording",
            limits.clone(),
            || Ok(()),
            |port| port.visit(&[], |_| Ok(())),
        )
        .unwrap();
    let mut connection = rusqlite::Connection::open(store.root().join("hunin.sqlite")).unwrap();
    let transaction = connection.transaction().unwrap();
    transaction.execute("INSERT INTO result_sets(id,run_id,input_identity,source_key,active) VALUES('old','recording','old','old',0)",[]).unwrap();
    transaction
        .execute("UPDATE observations SET timestamp_us=1000000", [])
        .unwrap();
    {
        let mut insert=transaction.prepare("INSERT INTO observations(run_id,id,kind,timestamp_us,result_set_id,record) VALUES('recording',?1,'event',?2,'old','{}')").unwrap();
        for index in 0..10000 {
            insert
                .execute(rusqlite::params![format!("old-{index}"), index])
                .unwrap();
        }
    }
    transaction.commit().unwrap();
    let mut visited = false;
    let result = store.with_query_snapshot(
        "recording",
        limits,
        || Ok(()),
        |port| {
            port.visit(&[], |_| {
                visited = true;
                Ok(())
            })
        },
    );
    let error = result.unwrap_err();
    let budget = error.downcast_ref::<ReadBudgetExceeded>().unwrap();
    assert_eq!(budget.dimension, ReadDimension::SqliteSteps);
    assert!(budget.observed > budget.limit);
    assert!(
        !visited,
        "history work ran outside the SQL budget until a selected row was yielded"
    );
}

#[test]
fn an_open_store_cannot_read_a_later_unsupported_schema() {
    let (_root, store) = fixture();
    let connection = rusqlite::Connection::open(store.root().join("hunin.sqlite")).unwrap();
    connection.execute_batch("PRAGMA user_version=99").unwrap();
    let mut consumed = false;
    let error = store
        .with_query_snapshot(
            "recording",
            ReadLimits::default(),
            || Ok(()),
            |_| {
                consumed = true;
                Ok(())
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("requires a newer Hunin"));
    assert!(!consumed);
    let schema: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(schema, 99);
}

#[test]
fn metric_selector_remains_usable_beside_many_unrelated_samples() {
    let (_root, store) = fixture();
    let mut connection = rusqlite::Connection::open(store.root().join("hunin.sqlite")).unwrap();
    let transaction = connection.transaction().unwrap();
    transaction.execute("INSERT INTO result_sets(id,run_id,input_identity,source_key,active) VALUES('wide','recording','wide','wide',1)",[]).unwrap();
    {
        let mut insert=transaction.prepare("INSERT INTO observations(run_id,id,kind,timestamp_us,result_set_id,record) VALUES('recording',?1,?2,?3,'wide',?4)").unwrap();
        for index in 0..10000 {
            let id = format!("sample-{index}");
            let record = json!({"schema_version":1,"kind":"sample","id":id,"run_id":"recording","producer_id":"fixture","pid":1,"tid":"thread","timestamp_us":index,"basis":"observed","weight":1,"weight_unit":"samples","stack":[{"function":"work"}]});
            insert
                .execute(rusqlite::params![
                    id,
                    "sample",
                    index,
                    serde_json::to_string(&record).unwrap()
                ])
                .unwrap();
        }
        for index in 0..2 {
            let id = format!("metric-{index}");
            let record = json!({"schema_version":1,"kind":"metric","id":id,"run_id":"recording","producer_id":"fixture","pid":1,"tid":"process","timestamp_us":index,"basis":"observed","name":"process.rss","unit":"bytes","scope":"process","value":index});
            insert
                .execute(rusqlite::params![
                    id,
                    "metric",
                    index,
                    serde_json::to_string(&record).unwrap()
                ])
                .unwrap();
        }
    }
    transaction.commit().unwrap();
    let limits = ReadLimits {
        maximum_sqlite_steps: 2000,
        ..Default::default()
    };
    let result = store.with_query_snapshot(
        "recording",
        limits.clone(),
        || Ok(()),
        |port| port.visit(&[], |_| Ok(())),
    );
    assert_eq!(
        result
            .unwrap_err()
            .downcast_ref::<ReadBudgetExceeded>()
            .unwrap()
            .dimension,
        ReadDimension::SqliteSteps
    );
    let snapshot = store
        .with_query_snapshot(
            "recording",
            limits,
            || Ok(()),
            |port| {
                let mut metrics = vec![];
                port.visit(&["metric"], |text| {
                    metrics.push(serde_json::from_str::<Value>(text)?);
                    Ok(())
                })?;
                Ok(hunin::query::resources_page(&metrics, &json!({})))
            },
        )
        .unwrap();
    assert_eq!(snapshot.value["returned_points"], 2);
    assert_eq!(snapshot.usage.rows, 2);
    assert!(snapshot.usage.sqlite_steps_approx <= 2000);
}

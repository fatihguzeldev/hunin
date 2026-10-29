use hunin::{
    model::runs::Run,
    storage::{ReadBudgetExceeded, ReadDimension, ReadLimits, Store},
};
use serde_json::{Value, json};

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

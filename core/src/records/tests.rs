use super::*;
use crate::query::{Query, QueryLimits};
use serde_json::json;

#[test]
fn sqlite_full_rolls_back_records_indexes_receipt_and_revision() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "p").unwrap();
    let investigation = project
        .create_investigation("i", "fault", json!({}))
        .unwrap();
    let run = project.create_run(&investigation, "r", json!({})).unwrap();
    let source = project
        .register_source(
            &run,
            SourceDefinition {
                id: "s".into(),
                target: None,
                schemas: vec![RecordSchema {
                    name: "s".into(),
                    version: 1,
                    description: "disk full fixture".into(),
                    fields: BTreeMap::from([(
                        "count".into(),
                        FieldSchema {
                            kind: FieldType::Unsigned,
                            required: true,
                            unit: Some("items".into()),
                            meaning: "fixture count".into(),
                        },
                    )]),
                }],
                clocks: vec![],
                interpretation: "fault-v1".into(),
                metadata: json!({}),
            },
        )
        .unwrap();
    let record = |id: &str, detail: Value| Record {
        id: id.into(),
        schema: "s".into(),
        schema_version: 1,
        subject: "test".into(),
        time: None,
        function: None,
        location: None,
        operation: None,
        payload: json!({"count":1,"detail":detail}),
    };
    source
        .append_batch("old", &[record("old", Value::Null)])
        .unwrap();
    let before = project.revision().unwrap();
    project
        .with_connection(|connection| {
            let pages: u64 = connection.query_row("PRAGMA page_count", [], |row| row.get(0))?;
            connection.pragma_update(None, "max_page_count", pages + 2)?;
            Ok(())
        })
        .unwrap();
    let failure = source
        .append_batch(
            "retry",
            &[
                record("first", Value::Null),
                record("large", json!("x".repeat(700000))),
            ],
        )
        .unwrap_err();
    assert!(
        matches!(failure,Error::Sqlite(rusqlite::Error::SqliteFailure(code,_)) if code.extended_code==rusqlite::ffi::SQLITE_FULL)
    );
    assert_eq!(project.revision().unwrap(), before);
    assert_eq!(
        project
            .query(&Query::new(run.clone()), QueryLimits::default(), None)
            .unwrap()
            .records
            .len(),
        1
    );
    project
        .with_connection(|connection| {
            for table in ["records", "record_fields", "delivery_batches"] {
                let count: i64 =
                    connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })?;
                assert_eq!(count, 1, "{table} has a partial failed delivery");
            }
            connection.pragma_update(None, "max_page_count", 2147483646_i64)?;
            Ok(())
        })
        .unwrap();
    source
        .append_batch(
            "retry",
            &[
                record("first", Value::Null),
                record("large", json!("x".repeat(700000))),
            ],
        )
        .unwrap();
    drop(source);
    drop(project);
    let project = Project::open(directory.path(), "p").unwrap();
    assert_eq!(
        project
            .query(&Query::new(run), QueryLimits::default(), None)
            .unwrap()
            .records
            .len(),
        3
    );
}

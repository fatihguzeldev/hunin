use hunin_core::{
    project::Project,
    query::{FieldFilter, Query, QueryLimits},
    records::{FieldSchema, FieldType, Record, RecordSchema, SourceDefinition},
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn source_queries_keep_a_snapshot_and_page_large_values_without_materializing_them() {
    let data = tempfile::tempdir().unwrap();
    let p = Project::create(data.path(), "p").unwrap();
    let i = p.create_investigation("i", "query", json!({})).unwrap();
    let r = p.create_run(&i, "r", json!({})).unwrap();
    let s = p
        .register_source(
            &r,
            SourceDefinition {
                id: "s".into(),
                target: None,
                schemas: vec![RecordSchema {
                    name: "counts".into(),
                    version: 1,
                    description: "exact source values".into(),
                    fields: BTreeMap::from([(
                        "count".into(),
                        FieldSchema {
                            kind: FieldType::Unsigned,
                            required: true,
                            unit: Some("events".into()),
                            meaning: "observed event count".into(),
                        },
                    )]),
                }],
                clocks: vec![],
                interpretation: "v1".into(),
                metadata: json!({}),
            },
        )
        .unwrap();
    let record = |id: String, n: u64| Record {
        id,
        schema: "counts".into(),
        schema_version: 1,
        subject: "process".into(),
        time: None,
        function: None,
        location: None,
        operation: None,
        payload: json!({"count":n}),
    };
    s.append(
        &(0..20)
            .map(|n| record(n.to_string(), n))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let query = Query::new(r.clone());
    let limits = QueryLimits {
        max_rows: 3,
        max_bytes: 4096,
    };
    let page = p.query(&query, limits, None).unwrap();
    let revision = page.revision;
    let mut ids = page
        .records
        .into_iter()
        .map(|x| x.reference.id)
        .collect::<BTreeSet<_>>();
    s.append(&[record("late".into(), 999)]).unwrap();
    let mut cursor = page.continuation;
    while let Some(next) = cursor {
        let page = p.query(&query, limits, Some(&next)).unwrap();
        assert_eq!(page.revision, revision);
        for row in page.records {
            assert!(ids.insert(row.reference.id));
        }
        cursor = page.continuation;
    }
    assert_eq!(ids, (0..20).map(|n| n.to_string()).collect());
    let mut filter = Query::new(r.clone());
    filter.fields.push(FieldFilter {
        name: "count".into(),
        value: json!(13),
    });
    assert_eq!(
        p.query(&filter, QueryLimits::default(), None)
            .unwrap()
            .records[0]
            .reference
            .id,
        "13"
    );
    filter.fields[0].name = "not-declared".into();
    assert!(p.query(&filter, QueryLimits::default(), None).is_err());
    let mut large = record("large".into(), 1000);
    large.payload["detail"] = json!("界".repeat(20000));
    s.append(std::slice::from_ref(&large)).unwrap();
    let mut query = Query::new(r);
    query.fields.push(FieldFilter {
        name: "count".into(),
        value: json!(1000),
    });
    let page = p
        .query(
            &query,
            QueryLimits {
                max_rows: 1,
                max_bytes: 1024,
            },
            None,
        )
        .unwrap();
    assert!(page.records[0].record.is_none());
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = p.read_value(&page.records[0].value, offset, 4096).unwrap();
        assert!(chunk.bytes.len() <= 4096);
        bytes.extend(chunk.bytes);
        match chunk.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    let actual: Record = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(actual, large);
}

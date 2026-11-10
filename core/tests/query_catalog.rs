use hunin_core::{
    project::{Error, Project, Reference},
    query::QueryLimits,
    records::{Clock, FieldSchema, FieldType, RecordSchema, SourceDefinition, SourceHandle},
};
use serde_json::json;
use std::collections::BTreeMap;
fn prepared_source(project: &Project) -> (Reference, SourceHandle) {
    let investigation = project
        .create_investigation("investigation", "preview", json!({}))
        .unwrap();
    let run = project
        .create_run(&investigation, "run", json!({"input":"fixture"}))
        .unwrap();
    let target = project
        .register_target(&run, "target", json!({"pid":42,"start":"instance-a"}), true)
        .unwrap();
    let definition = SourceDefinition {
        id: "source".into(),
        target: Some(target),
        schemas: vec![RecordSchema {
            name: "measure".into(),
            version: 1,
            description: "controlled counter".into(),
            fields: BTreeMap::from([(
                "count".into(),
                FieldSchema {
                    kind: FieldType::Unsigned,
                    required: true,
                    unit: Some("items".into()),
                    meaning: "completed fixture items".into(),
                },
            )]),
        }],
        clocks: vec![Clock {
            id: "clock-a".into(),
            ticks_per_second: 1000,
            description: "fixture source clock".into(),
        }],
        interpretation: "measure-v1".into(),
        metadata: json!({"scope":"process"}),
    };
    let source = project.register_source(&run, definition).unwrap();
    (run, source)
}

#[test]
fn reopened_catalogs_keep_snapshot_scope_status_and_bounded_values() {
    let dir = tempfile::tempdir().unwrap();
    let project = Project::create(dir.path(), "p").unwrap();
    let (run, source) = prepared_source(&project);
    let investigation = project.reference("investigation");
    let second = project
        .create_investigation("second", "second", json!({"detail":"🌒".repeat(30000)}))
        .unwrap();
    let limits = QueryLimits {
        max_rows: 1,
        max_bytes: 2048,
    };
    let first = project.investigations(limits, None).unwrap();
    let cursor = first.continuation.unwrap();
    project
        .create_investigation("later", "later", json!({}))
        .unwrap();
    let next = project.investigations(limits, Some(&cursor)).unwrap();
    assert_eq!(next.items[0].reference, second);
    assert!(next.continuation.is_none());
    assert!(next.items[0].metadata.value.is_none());
    assert!(serde_json::to_vec(&next).unwrap().len() <= limits.max_bytes);
    let chunk = project
        .read_value(&next.items[0].metadata.reference, 0, 1024)
        .unwrap();
    assert_eq!(chunk.bytes.len(), 1024);
    assert!(chunk.next_offset.is_some());
    assert!(matches!(
        project.runs(&investigation, limits, Some(&cursor)),
        Err(Error::Conflict(_))
    ));
    assert_eq!(
        project.runs(&investigation, limits, None).unwrap().items[0].reference,
        run
    );
    source.ready(json!({"covered":"scope"})).unwrap();
    source.report_gap("gap", json!({"lost":12})).unwrap();
    let mut definition = source.definition().unwrap();
    definition.id = "source-two".into();
    let source_two = project.register_source(&run, definition).unwrap();
    let first_sources = project.sources(&run, limits, None).unwrap();
    let source_cursor = first_sources.continuation.unwrap();
    source_two.close(json!({"complete":false})).unwrap();
    source_two.report_gap("closed", json!({})).unwrap_err();
    let second_old = project.sources(&run, limits, Some(&source_cursor)).unwrap();
    assert!(!second_old.items[0].closed);
    assert!(second_old.items[0].cutoff.is_none());
    let all = project.sources(&run, QueryLimits::default(), None).unwrap();
    assert_eq!(all.items[0].gap_count, 1);
    assert!(all.items[0].ready);
    assert!(all.items[1].closed);
    let events = project
        .source_events(&run, "source", QueryLimits::default(), None)
        .unwrap();
    assert_eq!(events.items.len(), 2);
    assert_eq!(events.items[1].metadata.value.as_ref().unwrap()["lost"], 12);
    drop(source_two);
    drop(source);
    drop(project);
    let reopened = Project::open(dir.path(), "p").unwrap();
    let investigations = reopened
        .investigations(QueryLimits::default(), None)
        .unwrap();
    assert_eq!(investigations.items.len(), 3);
    let runs = reopened
        .runs(
            &investigations.items[0].reference,
            QueryLimits::default(),
            None,
        )
        .unwrap();
    let sources = reopened
        .sources(&runs.items[0].reference, QueryLimits::default(), None)
        .unwrap();
    let schema = &sources.items[0].definition.value.as_ref().unwrap().schemas[0];
    assert!(schema.fields["count"].kind.supports_equality());
    assert_eq!(schema.fields["count"].unit.as_deref(), Some("items"));
    assert_eq!(
        reopened
            .source_events(&run, "source", QueryLimits::default(), None)
            .unwrap()
            .items[1]
            .kind,
        "gap"
    );
    let foreign = Project::create(dir.path(), "foreign").unwrap();
    assert!(matches!(
        foreign.investigations(limits, Some(&cursor)),
        Err(Error::ForeignProject)
    ));
}

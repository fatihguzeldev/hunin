use std::collections::BTreeMap;

use hunin_core::{
    project::{Error, Project, Reference},
    query::{
        Context, Direction, FieldFilter, Origin, Query, QueryLimits, Relationship, TimeFilter,
    },
    records::{
        Clock, FieldSchema, FieldType, Record, RecordSchema, RecordTime, SourceDefinition,
        SourceHandle,
    },
};
use serde_json::{Value, json};

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

fn record(id: &str, count: u64) -> Record {
    Record {
        id: id.into(),
        schema: "measure".into(),
        schema_version: 1,
        subject: "target".into(),
        time: Some(RecordTime {
            clock: "clock-a".into(),
            start: 10,
            end: Some(20),
            uncertainty: 1,
        }),
        function: Some("preparePreview".into()),
        location: Some("fixture.rs:12".into()),
        operation: Some("preview".into()),
        payload: json!({"count":count,"extra":{"preserved":true}}),
    }
}

#[test]
fn project_binding_source_lifetimes_and_exact_values_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let a = Project::create(dir.path(), "a").unwrap();
    let b = Project::create(dir.path(), "b").unwrap();
    a.set_metadata(json!({"repository":"same/path"})).unwrap();
    b.set_metadata(json!({"repository":"same/path"})).unwrap();
    let (run_a, source_a) = prepared_source(&a);
    let (run_b, source_b) = prepared_source(&b);
    let first = record("same-id", u64::MAX);
    let second = record("same-id", 17);
    source_a
        .append_batch("same-delivery", std::slice::from_ref(&first))
        .unwrap();
    source_b
        .append_batch("same-delivery", std::slice::from_ref(&second))
        .unwrap();
    let reference = source_a.record_ref("same-id");
    assert!(matches!(
        b.record(&reference, None),
        Err(Error::ForeignProject)
    ));
    let query_a = Query::new(run_a.clone());
    let first_page = a
        .query(
            &query_a,
            QueryLimits {
                max_rows: 1,
                max_bytes: 2048,
            },
            None,
        )
        .unwrap();
    assert_eq!(
        first_page.records[0].record.as_ref().unwrap().payload["count"],
        json!(u64::MAX)
    );
    let old_definition = source_a.definition().unwrap();
    let other_run = a
        .create_run(&a.reference("investigation"), "other-run", json!({}))
        .unwrap();
    a.register_target(
        &other_run,
        "target",
        json!({"pid":42,"start":"instance-b"}),
        true,
    )
    .unwrap();
    assert!(matches!(
        a.register_source(&other_run, old_definition),
        Err(Error::Conflict(_))
    ));
    a.set_metadata(json!({"repository":"moved/path"})).unwrap();
    drop(source_a);
    drop(a);
    let reopened = Project::open(dir.path(), "a").unwrap();
    assert_eq!(reopened.metadata().unwrap()["repository"], "moved/path");
    assert_eq!(reopened.record(&reference, None).unwrap(), first);
    assert_eq!(
        b.query(&Query::new(run_b), QueryLimits::default(), None)
            .unwrap()
            .records[0]
            .record
            .as_ref()
            .unwrap(),
        &second
    );
}

#[test]
fn small_profiler_durations_keep_their_exact_float_bits_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "p").unwrap();
    let (_, source) = prepared_source(&project);
    let mut expected = record("duration", 1);
    let seconds = 2.0076461125511958e-7_f64;
    expected.payload["seconds"] = json!(seconds);
    let reference = source.record_ref("duration");
    source
        .append_batch("duration", std::slice::from_ref(&expected))
        .unwrap();
    drop(source);
    drop(project);
    let reopened = Project::open(directory.path(), "p").unwrap();
    let actual = reopened.record(&reference, None).unwrap();
    assert_eq!(
        actual.payload["seconds"].as_f64().unwrap().to_bits(),
        seconds.to_bits()
    );
    assert_eq!(actual, expected);
}

#[test]
fn replay_conflict_and_invalid_batches_are_atomic_and_fenced() {
    let dir = tempfile::tempdir().unwrap();
    let project = Project::create(dir.path(), "p").unwrap();
    let (run, source) = prepared_source(&project);
    source
        .ready(json!({"coverage":"ready-before-work"}))
        .unwrap();
    let accepted = source
        .append_batch("batch", &[record("one", 1), record("two", 1)])
        .unwrap();
    assert_eq!((accepted.inserted, accepted.replayed), (2, 0));
    let revision = project.revision().unwrap();
    let replay = source
        .append_batch("batch", &[record("one", 1), record("two", 1)])
        .unwrap();
    assert_eq!((replay.inserted, replay.replayed), (0, 2));
    assert_eq!(project.revision().unwrap(), revision);
    assert!(matches!(
        source.append_batch("batch", &[record("one", 9)]),
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        source.append(&[record("three", 3), record("one", 9)]),
        Err(Error::Conflict(_))
    ));
    let mut wrong = record("invalid", 5);
    wrong.payload = json!({"count":"five"});
    assert!(matches!(
        source.append(&[record("four", 4), wrong]),
        Err(Error::Invalid(_))
    ));
    source
        .report_gap("gap-1", json!({"lost":null,"reason":"unknown tail"}))
        .unwrap();
    source.close(json!({"complete":false})).unwrap();
    assert!(matches!(
        source.append_batch("late-empty", &[]),
        Err(Error::Closed)
    ));
    assert!(matches!(
        source.append_batch("late-receipt", &[record("one", 1)]),
        Err(Error::Closed)
    ));
    assert_eq!(source.append(&[record("one", 1)]).unwrap().replayed, 1);
    assert!(matches!(
        source.append(&[record("late", 8)]),
        Err(Error::Closed)
    ));
    assert!(matches!(source.ready(json!({})), Err(Error::Closed)));
    assert!(source.status().unwrap().closed);
    let page = project
        .query(&Query::new(run), QueryLimits::default(), None)
        .unwrap();
    assert_eq!(page.records.len(), 2);
    drop(source);
    drop(project);
    let project = Project::open(dir.path(), "p").unwrap();
    let reopened = project.source(&project.reference("run"), "source").unwrap();
    assert_eq!(
        reopened
            .append_batch("batch", &[record("one", 1), record("two", 1)])
            .unwrap()
            .replayed,
        2
    );
    assert!(matches!(
        reopened.append(&[record("late", 8)]),
        Err(Error::Closed)
    ));
}

#[test]
fn keyset_views_filter_exact_fields_and_do_not_mix_new_records() {
    let dir = tempfile::tempdir().unwrap();
    let project = Project::create(dir.path(), "p").unwrap();
    let (run, source) = prepared_source(&project);
    let records: Vec<_> = (0..11).map(|n| record(&format!("item-{n}"), n)).collect();
    source.append(&records).unwrap();
    let mut query = Query::new(run.clone());
    query.function = Some("preparePreview".into());
    query.time = Some(TimeFilter {
        clock: "clock-a".into(),
        start: 12,
        end: 15,
    });
    let limits = QueryLimits {
        max_rows: 3,
        max_bytes: 8192,
    };
    let first = project.query(&query, limits, None).unwrap();
    let revision = first.revision;
    source.append(&[record("arrived-later", 99)]).unwrap();
    project
        .add_context(
            &run,
            Context {
                id: "label".into(),
                subject: "preview".into(),
                origin: Origin::Contributed,
                value: json!({"asset":"a.png"}),
                corrects: None,
            },
        )
        .unwrap();
    let mut all = first.records;
    let mut cursor = first.continuation;
    while let Some(token) = cursor {
        let page = project.query(&query, limits, Some(&token)).unwrap();
        assert_eq!(page.revision, revision);
        all.extend(page.records);
        cursor = page.continuation;
    }
    assert_eq!(all.len(), 11);
    assert!(
        all.iter()
            .all(|entry| entry.reference.id != "arrived-later")
    );
    let mut filtered = Query::new(run);
    filtered.fields.push(FieldFilter {
        name: "count".into(),
        value: json!(7),
    });
    assert_eq!(
        project
            .query(&filtered, QueryLimits::default(), None)
            .unwrap()
            .records[0]
            .reference
            .id,
        "item-7"
    );
    let token = project
        .query(&query, limits, None)
        .unwrap()
        .continuation
        .unwrap();
    let mut changed = query.clone();
    changed.function = Some("another".into());
    assert!(matches!(
        project.query(&changed, limits, Some(&token)),
        Err(Error::Conflict(_))
    ));
    let other = Project::create(dir.path(), "other").unwrap();
    let (other_run, _) = prepared_source(&other);
    assert!(matches!(
        other.query(&Query::new(other_run), limits, Some(&token)),
        Err(Error::ForeignProject)
    ));
}

#[test]
fn large_records_use_bounded_references_and_incremental_value_reads() {
    let dir = tempfile::tempdir().unwrap();
    let project = Project::create(dir.path(), "p").unwrap();
    let (run, source) = prepared_source(&project);
    let mut large = record("large", 8);
    large.payload["detail"] = json!("🌒".repeat(180000));
    source.append(std::slice::from_ref(&large)).unwrap();
    let page = project
        .query(
            &Query::new(run),
            QueryLimits {
                max_rows: 10,
                max_bytes: 2048,
            },
            None,
        )
        .unwrap();
    assert!(serde_json::to_vec(&page).unwrap().len() <= 2048);
    assert!(page.records[0].record.is_none());
    let reference = &page.records[0].value;
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = project.read_value(reference, offset, 65537).unwrap();
        assert!(chunk.bytes.len() <= 65537);
        bytes.extend(chunk.bytes);
        if let Some(next) = chunk.next_offset {
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(serde_json::from_slice::<Record>(&bytes).unwrap(), large);
    assert!(matches!(
        project.read_value(reference, reference.bytes + 1, 1),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn context_corrections_and_handoff_links_preserve_old_views() {
    let dir = tempfile::tempdir().unwrap();
    let project = Project::create(dir.path(), "p").unwrap();
    let (run, source) = prepared_source(&project);
    source
        .append(&[
            record("operation", 1),
            record("work", 2),
            record("unrelated", 3),
        ])
        .unwrap();
    source.close(json!({"complete":true})).unwrap();
    project
        .add_context(
            &run,
            Context {
                id: "a".into(),
                subject: "operation".into(),
                origin: Origin::Contributed,
                value: json!({"asset":"old.png"}),
                corrects: None,
            },
        )
        .unwrap();
    let old = project.revision().unwrap();
    project
        .add_context(
            &run,
            Context {
                id: "b".into(),
                subject: "operation".into(),
                origin: Origin::Contributed,
                value: json!({"asset":"new.png"}),
                corrects: Some("a".into()),
            },
        )
        .unwrap();
    assert_eq!(
        project
            .contexts(&run, Some(old), QueryLimits::default(), None)
            .unwrap()
            .contexts[0]
            .context
            .as_ref()
            .unwrap()
            .id,
        "a"
    );
    assert_eq!(
        project
            .contexts(&run, None, QueryLimits::default(), None)
            .unwrap()
            .contexts[0]
            .context
            .as_ref()
            .unwrap()
            .id,
        "b"
    );
    let operation = source.record_ref("operation");
    let work = source.record_ref("work");
    let link = Relationship {
        id: "handoff".into(),
        from: operation.clone(),
        to: work.clone(),
        kind: "causes".into(),
        origin: Origin::Instrumented,
        corrects: None,
        metadata: json!({"thread":"worker"}),
    };
    project.add_relationship(&run, link.clone()).unwrap();
    for (endpoint, direction) in [
        (&operation, Direction::Forward),
        (&work, Direction::Reverse),
    ] {
        let page = project
            .relationships(endpoint, direction, None, QueryLimits::default(), None)
            .unwrap();
        assert_eq!(page.relationships.len(), 1);
        assert!(page.relationships[0].from_present && page.relationships[0].to_present);
        assert_eq!(page.relationships[0].relationship.as_ref().unwrap(), &link);
    }
    assert!(
        project
            .relationships(
                &source.record_ref("unrelated"),
                Direction::Forward,
                None,
                QueryLimits::default(),
                None
            )
            .unwrap()
            .relationships
            .is_empty()
    );
    let reverse = Relationship {
        id: "cycle".into(),
        from: work,
        to: operation,
        kind: "causes".into(),
        origin: Origin::Captured,
        corrects: None,
        metadata: Value::Null,
    };
    assert!(matches!(
        project.add_relationship(&run, reverse),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn nullable_fields_are_declared_and_unsupported_predicates_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let project = Project::create(dir.path(), "p").unwrap();
    let (run, source) = prepared_source(&project);
    let mut definition = source.definition().unwrap();
    definition.id = "nullable".into();
    definition.schemas[0].fields.insert(
        "file".into(),
        FieldSchema {
            kind: FieldType::Nullable(Box::new(FieldType::String)),
            required: true,
            unit: None,
            meaning: "builtin has no source file".into(),
        },
    );
    definition.schemas[0].fields.insert(
        "children".into(),
        FieldSchema {
            kind: FieldType::Array,
            required: false,
            unit: None,
            meaning: "retained child references".into(),
        },
    );
    let source = project.register_source(&run, definition).unwrap();
    let mut value = record("builtin", 1);
    value.payload["file"] = Value::Null;
    value.payload["children"] = json!([1, 2]);
    source.append(&[value.clone()]).unwrap();
    let mut query = Query::new(run);
    query.source = Some("nullable".into());
    query.fields = vec![FieldFilter {
        name: "file".into(),
        value: Value::Null,
    }];
    assert_eq!(
        project
            .query(&query, QueryLimits::default(), None)
            .unwrap()
            .records
            .len(),
        1
    );
    for (name, value) in [
        ("unknown", json!(1)),
        ("extra", json!({"preserved":true})),
        ("children", json!([1, 2])),
        ("count", json!("one")),
    ] {
        query.fields = vec![FieldFilter {
            name: name.into(),
            value,
        }];
        assert!(matches!(
            project.query(&query, QueryLimits::default(), None),
            Err(Error::Invalid(_))
        ));
    }
    value.id = "missing".into();
    value.payload.as_object_mut().unwrap().remove("file");
    assert!(matches!(source.append(&[value]), Err(Error::Invalid(_))));
}

#[test]
fn equal_source_local_clock_names_cannot_imply_cross_source_alignment() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "p").unwrap();
    let (run, source) = prepared_source(&project);
    let mut definition = source.definition().unwrap();
    definition.id = "another".into();
    let another = project.register_source(&run, definition).unwrap();
    source.append(&[record("first", 1)]).unwrap();
    another.append(&[record("second", 2)]).unwrap();
    let mut query = Query::new(run);
    query.time = Some(TimeFilter {
        clock: "clock-a".into(),
        start: 10,
        end: 20,
    });
    assert!(matches!(
        project.query(&query, QueryLimits::default(), None),
        Err(Error::Invalid(_))
    ));
    query.source = Some("source".into());
    assert_eq!(
        project
            .query(&query, QueryLimits::default(), None)
            .unwrap()
            .records[0]
            .reference
            .id,
        "first"
    );
    query.time.as_mut().unwrap().clock = "unknown".into();
    assert!(matches!(
        project.query(&query, QueryLimits::default(), None),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn a_rejected_causal_batch_keeps_no_prefix_or_revision() {
    let dir = tempfile::tempdir().unwrap();
    let project = Project::create(dir.path(), "p").unwrap();
    let (run, source) = prepared_source(&project);
    let from = source.record_ref("unresolved-a");
    let to = source.record_ref("unresolved-b");
    let before = project.revision().unwrap();
    let a = Relationship {
        id: "a".into(),
        from: from.clone(),
        to: to.clone(),
        kind: "causes".into(),
        origin: Origin::Contributed,
        corrects: None,
        metadata: Value::Null,
    };
    let b = Relationship {
        id: "b".into(),
        from: to,
        to: from.clone(),
        ..a.clone()
    };
    assert!(matches!(
        project.add_relationships(&run, &[a, b]),
        Err(Error::Invalid(_))
    ));
    assert_eq!(project.revision().unwrap(), before);
    assert!(
        project
            .relationships(
                &from,
                Direction::Forward,
                None,
                QueryLimits::default(),
                None
            )
            .unwrap()
            .relationships
            .is_empty()
    );
}

#[test]
#[ignore = "measured 100k/1m SQLite operating-range check"]
fn query_scale_keeps_exact_sets_bounded_pages_and_stable_views() {
    use hunin_core::query::QueryControl;
    use std::time::{Duration, Instant};
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "scale").unwrap();
    let (run, source) = prepared_source(&project);
    let mut inserted = 0;
    let ingest = Instant::now();
    for count in [100000, 1000000] {
        while inserted < count {
            let end = (inserted + 256).min(count);
            let records: Vec<_> = (inserted..end)
                .map(|n| {
                    let mut record = record(&format!("item-{n}"), n as u64);
                    record.function = Some(format!("function-{}", n % 1000));
                    record.operation = Some(format!("operation-{}", n % 100));
                    record.time.as_mut().unwrap().start = n as i64;
                    record.time.as_mut().unwrap().end = Some(n as i64 + 1);
                    record
                })
                .collect();
            source.append(&records).unwrap();
            inserted = end;
        }
        let limits = QueryLimits {
            max_rows: 31,
            max_bytes: 32768,
        };
        let mut query = Query::new(run.clone());
        query.function = Some("function-7".into());
        let start = Instant::now();
        let first = project.query(&query, limits, None).unwrap();
        let revision = first.revision;
        let mut cursor = first.continuation;
        let mut ids = first
            .records
            .into_iter()
            .map(|entry| entry.reference.id)
            .collect::<Vec<_>>();
        let mut pages = 1;
        while let Some(token) = cursor {
            let page = project.query(&query, limits, Some(&token)).unwrap();
            assert_eq!(page.revision, revision);
            assert!(serde_json::to_vec(&page).unwrap().len() <= limits.max_bytes);
            ids.extend(page.records.into_iter().map(|entry| entry.reference.id));
            cursor = page.continuation;
            pages += 1;
        }
        let expected: Vec<_> = (7..count)
            .step_by(1000)
            .map(|n| format!("item-{n}"))
            .collect();
        assert_eq!(ids, expected);
        let function_time = start.elapsed();
        let mut exact = Query::new(run.clone());
        exact.fields.push(FieldFilter {
            name: "count".into(),
            value: json!(count - 1),
        });
        let start = Instant::now();
        let exact_page = project.query(&exact, limits, None).unwrap();
        assert_eq!(exact_page.records.len(), 1);
        assert_eq!(
            exact_page.records[0].reference.id,
            format!("item-{}", count - 1)
        );
        let field_time = start.elapsed();
        let mut timed = Query::new(run.clone());
        timed.time = Some(TimeFilter {
            clock: "clock-a".into(),
            start: count as i64 - 10,
            end: count as i64,
        });
        let start = Instant::now();
        let time_page = project.query(&timed, limits, None).unwrap();
        assert_eq!(time_page.records.len(), 11);
        let time_cost = start.elapsed();
        let mut slow = Query::new(run.clone());
        slow.subject = Some("absent-subject".into());
        let control = QueryControl {
            timeout: Duration::from_millis(1),
            ..QueryControl::default()
        };
        let start = Instant::now();
        assert!(matches!(
            project.query_with_control(&slow, limits, None, &control),
            Err(Error::Interrupted(_))
        ));
        let cancel_cost = start.elapsed();
        eprintln!(
            "storage-scale records={count} ingest_total_ms={} function_pages={pages} function_ms={} field_ms={} time_ms={} cancelled_ms={} sqlite={}",
            ingest.elapsed().as_millis(),
            function_time.as_millis(),
            field_time.as_millis(),
            time_cost.as_millis(),
            cancel_cost.as_millis(),
            rusqlite::version()
        );
    }
}

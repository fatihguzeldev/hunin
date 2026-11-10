use hunin_core::project::Project;
use hunin_core::records::{
    Clock, FieldSchema, FieldType, Record, RecordSchema, RecordTime, SourceDefinition,
};
use serde_json::json;
use std::collections::BTreeMap;

fn definition() -> SourceDefinition {
    SourceDefinition {
        id: "producer".into(),
        target: None,
        schemas: vec![RecordSchema {
            name: "counts".into(),
            version: 1,
            description: "source-specific exact counter".into(),
            fields: BTreeMap::from([(
                "count".into(),
                FieldSchema {
                    kind: FieldType::Unsigned,
                    required: true,
                    unit: Some("events".into()),
                    meaning: "observed completed events".into(),
                },
            )]),
        }],
        clocks: vec![Clock {
            id: "producer-clock".into(),
            ticks_per_second: 1_000_000,
            description: "source monotonic microseconds, not core arrival time".into(),
        }],
        interpretation: "counter-v1".into(),
        metadata: json!({"method":"instrumented call counter","coverage":"prepared operation"}),
    }
}

fn record(id: &str, count: u64) -> Record {
    Record {
        id: id.into(),
        schema: "counts".into(),
        schema_version: 1,
        subject: "worker-1".into(),
        time: Some(RecordTime {
            clock: "producer-clock".into(),
            start: 100,
            end: Some(110),
            uncertainty: 3,
        }),
        function: None,
        location: None,
        operation: Some("asset-1".into()),
        payload: json!({"count":count,"unknown_optional":{"label":"preserve me"}}),
    }
}

fn setup(
    root: &std::path::Path,
) -> (
    Project,
    hunin_core::project::Reference,
    hunin_core::records::SourceHandle,
) {
    let p = Project::create(root, "p").unwrap();
    let i = p.create_investigation("i", "delivery", json!({})).unwrap();
    let r = p.create_run(&i, "r", json!({})).unwrap();
    let s = p.register_source(&r, definition()).unwrap();
    (p, r, s)
}

#[test]
fn malformed_and_conflicting_batches_publish_nothing() {
    let root = tempfile::tempdir().unwrap();
    let (_p, _r, s) = setup(root.path());
    let first = record("first", 7);
    let mut invalid = record("invalid", 9);
    invalid.payload = json!({"count":"not an integer"});
    assert!(
        s.append_batch("invalid", &[first.clone(), invalid])
            .is_err()
    );
    assert_eq!(
        s.append_batch("valid", std::slice::from_ref(&first))
            .unwrap()
            .inserted,
        1
    );
    let new = record("new", 7);
    let conflict = record("first", 99);
    assert!(
        s.append_batch("conflict", &[new.clone(), conflict])
            .is_err()
    );
    assert_eq!(s.append(std::slice::from_ref(&new)).unwrap().inserted, 1);
    assert_eq!(s.append(std::slice::from_ref(&new)).unwrap().replayed, 1);
    assert_eq!(
        s.append(&[record("equal-payload-new-identity", 7)])
            .unwrap()
            .inserted,
        1
    );
}

#[test]
fn reopen_preserves_meaning_and_closed_cutoff_but_resolves_known_replay() {
    let root = tempfile::tempdir().unwrap();
    let (p, r, s) = setup(root.path());
    let expected = record("first", u64::MAX);
    let outcome = s
        .append_batch("delivery", std::slice::from_ref(&expected))
        .unwrap();
    s.report_gap(
        "gap-1",
        json!({"lost_count":null,"reason":"unknown interval"}),
    )
    .unwrap();
    let cutoff = s
        .close(json!({"complete":false,"reason":"interrupted"}))
        .unwrap();
    assert!(cutoff >= outcome.revision);
    drop(s);
    drop(p);
    let p = Project::open(root.path(), "p").unwrap();
    let s = p.source(&r, "producer").unwrap();
    assert_eq!(s.definition().unwrap(), definition());
    assert_eq!(s.append_batch("delivery", &[expected]).unwrap().replayed, 1);
    assert!(s.append(&[record("late", 1)]).is_err());
    assert!(s.ready(json!({"late":true})).is_err());
    assert!(s.report_gap("late-gap", json!({})).is_err());
    assert!(s.status().unwrap().closed);
}

#[test]
fn target_reference_from_another_run_cannot_rebind_equal_local_ids() {
    let root = tempfile::tempdir().unwrap();
    let (p, r, _s) = setup(root.path());
    let i = p.reference("i");
    let other = p.create_run(&i, "other", json!({})).unwrap();
    let a = p
        .register_target(&r, "same-pid-label", json!({"start":1}), false)
        .unwrap();
    p.register_target(&other, "same-pid-label", json!({"start":2}), false)
        .unwrap();
    let mut source = definition();
    source.id = "foreign-target".into();
    source.target = Some(a);
    assert!(p.register_source(&other, source).is_err());
}

#[test]
fn unsupported_clock_and_schema_do_not_become_valid_by_arrival() {
    let root = tempfile::tempdir().unwrap();
    let (_p, _r, s) = setup(root.path());
    let mut bad = record("wrong-clock", 1);
    bad.time.as_mut().unwrap().clock = "another-clock".into();
    assert!(s.append(&[bad]).is_err());
    let mut bad = record("wrong-schema", 1);
    bad.schema_version = 2;
    assert!(s.append(&[bad]).is_err());
    assert_eq!(s.append(&[record("valid", 1)]).unwrap().inserted, 1);
}

use super::*;
use crate::model::runs::Run;
use serde_json::json;

fn fixture() -> (tempfile::TempDir, Store, std::path::PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::open(&temporary.path().join("store")).unwrap();
    let directory = temporary.path().join("sdk");
    fs::create_dir(&directory).unwrap();
    let run: Run = serde_json::from_value(json!({
        "id":"sdk-budget","name":"sdk-budget","status":"completed", "command":[],
        "cwd":temporary.path(),"runtime":"none","started_at_us":0,
        "diagnostics":[],"source":{},"correctness":{},
    }))
    .unwrap();
    store.save_run(&run).unwrap();
    (temporary, store, directory)
}

fn row(id: &str) -> String {
    json!({"schema_version":1,"kind":"event","id":id,"run_id":"sdk-budget",
        "producer_id":"declared","pid":1,"tid":"","timestamp_us":0,
        "basis":"explicit","name":"budget-control"})
    .to_string()
}

fn outcome(store: &Store, directory: &Path, limits: SpoolBudget) -> Value {
    serde_json::to_value(ingest_with_budget(store, "sdk-budget", directory, limits)).unwrap()
}

fn assert_budget_rejection(value: &Value, store: &Store) {
    assert_eq!(value["status"], "rejected", "{value}");
    assert_eq!(value["stage"], "budget", "{value}");
    assert_eq!(value["evidence"]["completion"], "unknown");
    assert_eq!(value["evidence"]["loss"], "unknown");
    assert!(store.records("sdk-budget").unwrap().is_empty());
}

#[test]
fn inventory_limits_stop_without_claiming_complete_discovery() {
    for limit_entries in [true, false] {
        let (_temporary, store, directory) = fixture();
        fs::write(directory.join("a.jsonl"), row("a")).unwrap();
        fs::write(
            directory.join(if limit_entries {
                "unrelated.txt"
            } else {
                "b.jsonl"
            }),
            row("b"),
        )
        .unwrap();
        let mut limits = SpoolBudget::default();
        if limit_entries {
            limits.max_directory_entries = 1;
        } else {
            limits.max_spool_files = 1;
        }
        let value = outcome(&store, &directory, limits);
        assert_budget_rejection(&value, &store);
        assert_eq!(
            value["evidence"]["spool_snapshot"]["inventory_retained"],
            false
        );
        assert_eq!(
            value["evidence"]["budget"]["exceeded"]["limit"],
            if limit_entries {
                "directory_entries"
            } else {
                "spool_files"
            }
        );
        assert_eq!(value["evidence"]["budget"]["exceeded"]["observed"], 2);
    }
}

#[test]
fn actual_read_and_physical_line_limits_retain_the_full_raw_spool() {
    for limit_bytes in [true, false] {
        let (_temporary, store, directory) = fixture();
        let bytes = format!("{}\n{}", row("prefix"), " ".repeat(1000));
        fs::write(directory.join("a.jsonl"), bytes.as_bytes()).unwrap();
        let mut limits = SpoolBudget::default();
        if limit_bytes {
            limits.max_read_bytes = 10;
        } else {
            limits.max_line_bytes = row("prefix").len() as u64 + 1;
        }
        let value = outcome(&store, &directory, limits);
        assert_budget_rejection(&value, &store);
        let artifact = store.artifacts("sdk-budget").unwrap().pop().unwrap();
        assert_eq!(
            fs::read(store.artifact_path("sdk-budget", &artifact.id).unwrap()).unwrap(),
            bytes.as_bytes()
        );
        if limit_bytes {
            assert!(
                value["evidence"]["budget"]["work"]["read_bytes"]
                    .as_u64()
                    .unwrap()
                    == 11
            );
        } else {
            assert_eq!(
                value["evidence"]["budget"]["work"]["max_physical_line_bytes"],
                limits.max_line_bytes + 1
            );
        }
    }
}

#[test]
fn actual_read_budget_is_shared_across_retained_artifacts() {
    let (_temporary, store, directory) = fixture();
    let first = row("a");
    fs::write(directory.join("a.jsonl"), &first).unwrap();
    fs::write(directory.join("b.jsonl"), row("b")).unwrap();
    let limit = first.len() as u64 + 2;
    let value = outcome(
        &store,
        &directory,
        SpoolBudget {
            max_read_bytes: limit,
            ..SpoolBudget::default()
        },
    );
    assert_budget_rejection(&value, &store);
    assert_eq!(
        value["evidence"]["budget"]["exceeded"]["limit"],
        "read_bytes"
    );
    assert_eq!(value["evidence"]["budget"]["work"]["read_bytes"], limit + 1);
    assert_eq!(
        value["evidence"]["spool_snapshot"]["retained_artifact_count"],
        2
    );
    assert_eq!(store.artifacts("sdk-budget").unwrap().len(), 2);
}

#[test]
fn record_limit_never_publishes_an_accepted_prefix() {
    let (_temporary, store, directory) = fixture();
    fs::write(
        directory.join("a.jsonl"),
        format!("{}\n{}\n", row("a"), row("b")),
    )
    .unwrap();
    let value = outcome(
        &store,
        &directory,
        SpoolBudget {
            max_records: 1,
            ..SpoolBudget::default()
        },
    );
    assert_budget_rejection(&value, &store);
    assert_eq!(
        value["evidence"]["spool_snapshot"]["inventory_retained"],
        true
    );
}

#[test]
fn exact_limits_and_legacy_final_json_without_lf_remain_accepted() {
    for with_lf in [true, false] {
        let (_temporary, store, directory) = fixture();
        let bytes = format!("{}{}", row("exact"), if with_lf { "\n" } else { "" });
        fs::write(directory.join("a.jsonl"), bytes.as_bytes()).unwrap();
        let value = outcome(
            &store,
            &directory,
            SpoolBudget {
                max_directory_entries: 1,
                max_spool_files: 1,
                // The final-byte observation is an actual retained-artifact read.
                max_read_bytes: bytes.len() as u64 + 1,
                max_line_bytes: bytes.len() as u64,
                max_records: 1,
            },
        );
        assert_eq!(value["status"], "accepted", "{value}");
        assert_eq!(store.records("sdk-budget").unwrap().len(), 1);
    }
}

#[test]
fn expired_shared_deadline_rejects_before_decode_without_publishing_or_changing_correctness() {
    for empty in [false, true] {
        let (_temporary, store, directory) = fixture();
        let mut run = store.run("sdk-budget").unwrap();
        run.correctness = json!({"passed":true,"validation":"exit_code_only"});
        store.save_run(&run).unwrap();
        if !empty {
            fs::write(directory.join("a.jsonl"), row("not-published")).unwrap();
        }
        let value = serde_json::to_value(ingest_controlled(
            &store,
            "sdk-budget",
            &directory,
            Some(std::time::Instant::now() - std::time::Duration::from_millis(1)),
        ))
        .unwrap();
        assert_budget_rejection(&value, &store);
        assert_eq!(
            value["evidence"]["spool_snapshot"]["inventory_retained"],
            false
        );
        assert!(store.artifacts("sdk-budget").unwrap().is_empty());
        assert_eq!(
            store.run("sdk-budget").unwrap().correctness,
            run.correctness
        );
        if !empty {
            assert_eq!(
                fs::read_to_string(directory.join("a.jsonl")).unwrap(),
                row("not-published")
            );
        }
    }
}

#[test]
fn raw_retention_file_and_aggregate_caps_preserve_originals_without_accepting_a_prefix() {
    for per_file in [true, false] {
        let (_temporary, store, directory) = fixture();
        let first = row("a");
        let second = row("b");
        fs::write(directory.join("a.jsonl"), &first).unwrap();
        fs::write(directory.join("b.jsonl"), &second).unwrap();
        let raw = RawRetention {
            maximum_file_bytes: if per_file {
                first.len() as u64 - 1
            } else {
                1024 * 1024
            },
            maximum_read_bytes: if per_file {
                1024 * 1024
            } else {
                (first.len() + second.len()) as u64 - 1
            },
            read_bytes: 0,
        };
        let value = serde_json::to_value(ingest_with_policy(
            &store,
            "sdk-budget",
            &directory,
            SpoolBudget::default(),
            raw,
            None,
        ))
        .unwrap();
        assert_budget_rejection(&value, &store);
        assert_eq!(
            value["evidence"]["spool_snapshot"]["inventory_retained"],
            false
        );
        assert_eq!(
            value["evidence"]["raw_retention"]["read_bytes"],
            if per_file { 0 } else { first.len() }
        );
        assert_eq!(
            store.artifacts("sdk-budget").unwrap().len(),
            if per_file { 0 } else { 1 }
        );
        assert_eq!(
            fs::read_to_string(directory.join("a.jsonl")).unwrap(),
            first
        );
        assert_eq!(
            fs::read_to_string(directory.join("b.jsonl")).unwrap(),
            second
        );
        assert!(
            fs::read_dir(store.root().join("artifacts"))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".pending-"))
        );
    }
}

#[test]
fn exact_raw_retention_cap_preserves_legacy_final_json_without_lf() {
    let (_temporary, store, directory) = fixture();
    let bytes = row("exact-raw");
    fs::write(directory.join("a.jsonl"), &bytes).unwrap();
    let raw = RawRetention {
        maximum_file_bytes: bytes.len() as u64,
        maximum_read_bytes: bytes.len() as u64,
        read_bytes: 0,
    };
    let value = serde_json::to_value(ingest_with_policy(
        &store,
        "sdk-budget",
        &directory,
        SpoolBudget::default(),
        raw,
        None,
    ))
    .unwrap();
    assert_eq!(value["status"], "accepted", "{value}");
    assert_eq!(
        value["evidence"]["raw_retention"]["read_bytes"],
        bytes.len()
    );
    assert_eq!(value["evidence"]["completion"], "unknown");
    assert_eq!(value["evidence"]["loss"], "unknown");
    assert_eq!(store.records("sdk-budget").unwrap().len(), 1);
}

#[test]
fn decoded_sdk_rows_are_not_published_after_an_expired_deadline() {
    let (_temporary, store, directory) = fixture();
    let bytes = row("decoded");
    fs::write(directory.join("a.jsonl"), &bytes).unwrap();
    let mut evidence = SpoolEvidence::default();
    let records = read(
        &store,
        "sdk-budget",
        &directory,
        &mut evidence.spool_snapshot,
        &mut evidence.budget,
        &mut evidence.raw_retention,
        None,
    )
    .unwrap();
    assert_eq!(records.len(), 1);
    assert!(store.records("sdk-budget").unwrap().is_empty());
    // The natural decode/publication boundary receives an already-expired
    // shared deadline; no scheduler sleep or environment hook is required.
    let status = publish_records(
        &store,
        "sdk-budget",
        &records,
        &mut evidence,
        Some(Instant::now() - std::time::Duration::from_millis(1)),
    );
    let value = serde_json::to_value(IngestionOutcome { status, evidence }).unwrap();
    assert_eq!(value["status"], "rejected");
    assert_eq!(value["stage"], "acceptance");
    assert_eq!(
        value["evidence"]["spool_snapshot"]["inventory_retained"],
        true
    );
    assert!(store.records("sdk-budget").unwrap().is_empty());
    assert!(
        store
            .selected_result_id("sdk-budget", "sdk-spool-v1")
            .unwrap()
            .is_none()
    );
    let artifact = store.artifacts("sdk-budget").unwrap().pop().unwrap();
    assert_eq!(
        fs::read_to_string(store.artifact_path("sdk-budget", &artifact.id).unwrap()).unwrap(),
        bytes
    );
}

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

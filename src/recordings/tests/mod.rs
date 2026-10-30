use crate::storage::Store;
use serde_json::json;
use std::{fs, path::Path};

fn fixture() -> (tempfile::TempDir, Store, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("store")).unwrap();
    store.save_run(&serde_json::from_value(json!({"id":"bounded-import","name":"raw","status":"completed","command":[],"cwd":".","runtime":"imported","started_at_us":1,"finished_at_us":2,"exit_code":0,"diagnostics":[],"source":{},"correctness":{}})).unwrap()).unwrap();
    let path = root.path().join("raw.jsonl");
    fs::write(&path, format!("{}\n", json!({"schema_version":1,"kind":"event","id":"event","run_id":"bounded-import","producer_id":"fixture","pid":1,"tid":"main","timestamp_us":1,"basis":"observed","name":"bounded"}))).unwrap();
    (root, store, path)
}

mod publication;
mod reanalysis;

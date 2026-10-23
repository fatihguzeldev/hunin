use hunin::{model::runs::Run, storage::Store};
use serde_json::{Value, json};

fn run() -> Run {
    Run {
        id: "run-1".into(),
        name: "control".into(),
        status: "completed".into(),
        command: vec!["control".into()],
        cwd: "/tmp".into(),
        runtime: "native".into(),
        started_at_us: 1,
        finished_at_us: Some(5),
        exit_code: Some(0),
        diagnostics: vec![],
        source: json!({"fingerprint":"source-1"}),
        correctness: json!({"passed":true}),
    }
}
fn event(id: &str) -> Value {
    json!({"schema_version":1,"kind":"event","id":id,"run_id":"run-1","producer_id":"process-lifetime-1","pid":123,"tid":"main","timestamp_us":2,"name":"control","basis":"observed"})
}

#[test]
fn accepted_inputs_are_atomic_idempotent_and_survive_reopening() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let a = event("a");
    let mut bad = event("bad");
    bad["parent_id"] = json!("missing");
    assert!(
        store
            .accept("run-1", "input-bad", &[a.clone(), bad])
            .is_err()
    );
    assert!(store.records("run-1").unwrap().is_empty());
    assert!(store.accept("run-1", "input-a", &[a.clone()]).unwrap());
    assert!(!store.accept("run-1", "input-a", &[a]).unwrap());
    drop(store);
    let reopened = Store::open(root.path()).unwrap();
    assert_eq!(reopened.run("run-1").unwrap().name, "control");
    assert_eq!(reopened.records("run-1").unwrap().len(), 1);
    let mut cross_run = event("other");
    cross_run["run_id"] = json!("run-2");
    assert!(reopened.accept("run-1", "wrong-run", &[cross_run]).is_err());
}

#[test]
fn shared_raw_artifacts_survive_removing_one_run() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let first = run();
    store.save_run(&first).unwrap();
    let mut second = run();
    second.id = "run-2".into();
    store.save_run(&second).unwrap();
    let source = root.path().join("raw.json");
    std::fs::write(&source, b"immutable input").unwrap();
    let a = store.add_artifact("run-1", &source, "control").unwrap();
    let b = store.add_artifact("run-2", &source, "control").unwrap();
    assert_eq!(a.sha256, b.sha256);
    store.remove_run("run-1").unwrap();
    assert_eq!(
        std::fs::read(store.artifact_path("run-2", &b.id).unwrap()).unwrap(),
        b"immutable input"
    );
}

#[test]
fn distinct_artifact_names_share_bytes_without_losing_their_roles() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let stdout = root.path().join("stdout.txt");
    let stderr = root.path().join("stderr.txt");
    std::fs::write(&stdout, b"").unwrap();
    std::fs::write(&stderr, b"").unwrap();
    let a = store.add_artifact("run-1", &stdout, "stdout").unwrap();
    let b = store.add_artifact("run-1", &stderr, "stderr").unwrap();
    assert_eq!(a.sha256, b.sha256);
    assert_ne!(a.id, b.id);
    assert_eq!(store.artifacts("run-1").unwrap().len(), 2);
}

#[test]
fn references_preserve_kind_and_acyclic_parent_context_ownership() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let mut a = event("a");
    a["context_id"] = json!("b");
    let b = event("b");
    assert!(store.accept("run-1", "bad-context", &[a, b]).is_err());
    let mut a = event("a");
    a["kind"] = json!("span");
    a["duration_us"] = json!(1);
    a["parent_id"] = json!("b");
    let mut b = a.clone();
    b["id"] = json!("b");
    b["parent_id"] = json!("a");
    assert!(store.accept("run-1", "parent-cycle", &[a, b]).is_err());
}

use super::*;
use serde_json::json;

fn store(directory: &Path) -> Store {
    let store = Store::open(directory).unwrap();
    store.save_run(&serde_json::from_value(json!({"id":"raw-budget","name":"raw","status":"completed","command":[],"cwd":".","runtime":"none","started_at_us":1,"finished_at_us":2,"exit_code":0,"diagnostics":[],"source":{},"correctness":{}})).unwrap()).unwrap();
    store
}

#[test]
fn artifact_path_refuses_corrupt_retained_digest_metadata() {
    let root = tempfile::tempdir().unwrap();
    let store = store(root.path());
    let artifact = store
        .add_artifact_bytes("raw-budget", "raw.bin", "raw", b"bytes")
        .unwrap();
    let mut metadata = serde_json::to_value(&artifact).unwrap();
    metadata["sha256"] = json!("../../outside");
    store
        .connection()
        .unwrap()
        .execute(
            "UPDATE artifacts SET metadata=?1 WHERE run_id=?2 AND id=?3",
            params![metadata.to_string(), "raw-budget", artifact.id],
        )
        .unwrap();
    assert!(
        store
            .artifact_path("raw-budget", &artifact.id)
            .unwrap_err()
            .to_string()
            .contains("canonical SHA-256")
    );
}

use hunin::{
    model::runs::{Run, RunStatus},
    storage::Store,
};
use serde_json::json;

#[test]
fn known_recording_states_keep_the_existing_wire_strings() {
    for (status, wire) in [
        (RunStatus::Running, "running"),
        (RunStatus::Completed, "completed"),
        (RunStatus::Failed, "failed"),
        (RunStatus::Canceled, "canceled"),
    ] {
        assert_eq!(serde_json::to_value(&status).unwrap(), json!(wire));
        assert_eq!(
            serde_json::from_value::<RunStatus>(json!(wire)).unwrap(),
            status
        );
    }
}

#[test]
fn retained_unknown_status_survives_reopening_without_becoming_a_known_outcome() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = json!({
        "id":"historical", "name":"retained recording", "status":"legacy-partial",
        "command":[], "cwd":"/removed", "runtime":"imported", "started_at_us":1,
        "finished_at_us":2, "exit_code":null, "diagnostics":[],
        "source":{"fingerprint":"old-source"}, "correctness":{"passed":null}
    });
    let run: Run = serde_json::from_value(manifest.clone()).unwrap();
    assert_eq!(run.status, RunStatus::Unknown("legacy-partial".into()));
    assert_ne!(run.status, RunStatus::Completed);
    let store = Store::open(directory.path()).unwrap();
    store.save_run(&run).unwrap();
    drop(store);
    let reopened = Store::open(directory.path()).unwrap().run(&run.id).unwrap();
    assert_eq!(serde_json::to_value(reopened).unwrap(), manifest);
}

#[test]
fn status_wire_rejects_non_strings_instead_of_coercing_them() {
    for value in [json!(null), json!(1), json!({"status":"completed"})] {
        assert!(serde_json::from_value::<RunStatus>(value).is_err());
    }
}

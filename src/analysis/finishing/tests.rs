use super::*;
use crate::storage::Store;
use std::fs;
use tokio::sync::watch;

#[test]
fn expired_or_cancelled_retention_cannot_publish_an_otherwise_valid_result() {
    for (cancelled, expired, oversized, status) in [
        (false, true, false, "timed_out"),
        (true, false, false, "cancelled"),
        (false, false, true, "failed"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("store")).unwrap();
        store
            .save_run(&crate::model::runs::Run {
                id: "recording".into(),
                name: "finishing gate".into(),
                status: "completed".into(),
                command: vec![],
                cwd: "/fixture".into(),
                runtime: "fixture".into(),
                started_at_us: 1,
                finished_at_us: Some(2),
                exit_code: Some(0),
                diagnostics: vec![],
                source: json!({}),
                correctness: json!({}),
            })
            .unwrap();
        let event = json!({"schema_version":1,"kind":"event","id":"input","run_id":"recording","producer_id":"fixture","pid":1,"tid":"thread","timestamp_us":1,"basis":"observed","name":"input"});
        store
            .accept("recording", "input", std::slice::from_ref(&event))
            .unwrap();
        let mut derived = event.clone();
        derived["id"] = json!("derived");
        derived["basis"] = json!("derived");
        derived["evidence"] = json!(["input"]);
        let directory = root.path().join("attempt");
        fs::create_dir(&directory).unwrap();
        fs::write(
            directory.join("stdout.jsonl"),
            serde_json::to_vec(&derived).unwrap(),
        )
        .unwrap();
        let document = json!({"id":"attempt","run_id":"recording","status":"running","diagnostics":[],"phase_timings_us":{},"execution_dir":directory});
        if oversized {
            fs::OpenOptions::new()
                .write(true)
                .open(directory.join("stdout.jsonl"))
                .unwrap()
                .set_len(retention::Limits::default().bytes + 1)
                .unwrap();
        }
        store
            .save_document("analysis-results", "attempt", &document)
            .unwrap();
        let (_sender, cancellation) = watch::channel(cancelled);
        let now = Instant::now();
        let deadline = if expired {
            now - std::time::Duration::from_secs(1)
        } else {
            now + std::time::Duration::from_secs(30)
        };
        let control = process::Control::new(cancellation, deadline);
        let input = Input {
            bytes: vec![],
            fingerprint: "input-identity".into(),
            run_id: "recording".into(),
            result_ids: vec![],
            record_ids: ["input".into()].into_iter().collect(),
            artifacts: Default::default(),
        };
        let engine = Engine::new(store.clone());
        engine.finish(
            &json!({"id":"analysis","package_digest":"package"}),
            &input,
            document,
            &directory,
            &control,
            Ok(vec![derived]),
        );
        let document = store.document("analysis-results", "attempt").unwrap();
        assert_eq!(document["status"], status);
        assert_eq!(document["raw_retention"]["complete"], false);
        assert_eq!(store.records("recording").unwrap(), vec![event]);
        assert!(
            store
                .selected_result_id("recording", "analysis:analysis")
                .unwrap()
                .is_none()
        );
        assert!(directory.join("stdout.jsonl").is_file());
    }
}

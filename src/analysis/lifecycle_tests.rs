use super::*;
use crate::storage::Store;
use tokio::sync::watch;

fn store(root: &Path) -> Store {
    let store = Store::open(root).unwrap();
    store
        .save_run(&crate::model::runs::Run {
            id: "recording".into(),
            name: "wait ordering".into(),
            status: "completed".into(),
            command: vec![],
            cwd: root.display().to_string(),
            runtime: "fixture".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: Some(0),
            diagnostics: vec![],
            source: json!({}),
            correctness: json!({}),
        })
        .unwrap();
    store
}

#[tokio::test]
async fn terminal_commit_before_active_removal_overrides_a_stale_running_read() {
    let root = tempfile::tempdir().unwrap();
    let store = store(root.path());
    let engine = Engine::new(store.clone());
    let running =
        json!({"id":"attempt", "run_id":"recording", "status":"running", "owner":engine.owner});
    store
        .save_document("analysis-results", "attempt", &running)
        .unwrap();
    let (sender, _) = watch::channel(false);
    engine.active.lock().await.insert("attempt".into(), sender);

    // Force wait's old read to precede completion's actual durable terminal
    // write, and observe the same commit-before-map-removal order as admission.
    let stale = store.document("analysis-results", "attempt").unwrap();
    let mut terminal = running;
    terminal["status"] = json!("accepted");
    store
        .save_document("analysis-results", "attempt", &terminal)
        .unwrap();
    engine.active.lock().await.remove("attempt");
    assert_eq!(
        engine.wait_iteration("attempt", stale).await.unwrap(),
        Some(terminal)
    );
}

#[tokio::test]
async fn missing_owned_task_with_a_fresh_running_document_still_fails() {
    let root = tempfile::tempdir().unwrap();
    let store = store(root.path());
    let engine = Engine::new(store.clone());
    let running =
        json!({"id":"attempt", "run_id":"recording", "status":"running", "owner":engine.owner});
    store
        .save_document("analysis-results", "attempt", &running)
        .unwrap();
    let error = engine.wait_iteration("attempt", running).await.unwrap_err();
    assert!(error.to_string().contains("without a terminal result"));
}

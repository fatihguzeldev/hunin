use super::*;
use serde_json::json;

#[test]
fn publication_waiting_for_the_writer_cannot_outlive_its_deadline() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let run = Run {
        id: "recording".into(),
        name: "deadline control".into(),
        status: RunStatus::Completed,
        command: vec!["fixture".into()],
        cwd: ".".into(),
        runtime: "none".into(),
        started_at_us: 1,
        finished_at_us: Some(2),
        exit_code: Some(0),
        diagnostics: vec![],
        source: json!({}),
        correctness: json!({"passed":true}),
    };
    store.save_run(&run).unwrap();
    let event = |id| json!({"schema_version":1,"kind":"event","id":id,"run_id":"recording","producer_id":"fixture","pid":1,"tid":"main","timestamp_us":1,"name":"control","basis":"observed"});
    store
        .accept("recording", "source", &[event("before")])
        .unwrap();
    store
        .save_document(
            "analysis-results",
            "attempt",
            &json!({"run_id":"recording","status":"running"}),
        )
        .unwrap();
    let mut blocker = store.connection().unwrap();
    let lock = blocker
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let publishing = store.clone();
    let (ready, waiting) = std::sync::mpsc::channel();
    let task = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_millis(25);
        ready.send(()).unwrap();
        publishing.publish_result(PublicationRequest {
            run_id: "recording",
            source_key: "source",
            input_identity: "expired-result",
            records: &[event("after")],
            metadata: Some(PublicationMetadata::Analysis {
                execution_id: "attempt",
                document: &json!({"run_id":"recording","status":"accepted"}),
            }),
            deadline: Some(deadline),
        })
    });
    waiting.recv().unwrap();
    std::thread::sleep(Duration::from_millis(75));
    lock.commit().unwrap();
    let error = task
        .join()
        .unwrap()
        .expect_err("expired publication was accepted after waiting for the writer");
    assert!(error.to_string().contains("deadline"), "{error:#}");
    assert_eq!(store.records("recording").unwrap(), vec![event("before")]);
    assert_eq!(
        store.document("analysis-results", "attempt").unwrap()["status"],
        "running"
    );
}

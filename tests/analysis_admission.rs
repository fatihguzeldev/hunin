use hunin::{analysis::Engine, model::runs::Run, storage::Store};
use serde_json::{Value, json};
use std::{fs, sync::mpsc, time::Duration};

fn python() -> String {
    std::env::var("HUNIN_TEST_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into())
}

struct Fixture {
    _root: tempfile::TempDir,
    store: Store,
    engine: Engine,
    analysis: Value,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("store")).unwrap();
        store
            .save_run(&Run {
                id: "recording".into(),
                name: "admission control".into(),
                status: "completed".into(),
                command: vec![],
                cwd: root.path().display().to_string(),
                runtime: "fixture".into(),
                started_at_us: 1,
                finished_at_us: Some(2),
                exit_code: Some(0),
                diagnostics: vec![],
                source: json!({"fingerprint":"build-v1"}),
                correctness: json!({"passed":true}),
            })
            .unwrap();
        let package = root.path().join("package");
        fs::create_dir(&package).unwrap();
        fs::write(package.join("analyze.py"), "import time\ntime.sleep(60)\n").unwrap();
        let engine = Engine::with_admission_limit(store.clone(), 1).unwrap();
        let analysis = engine
            .register(&json!({
                "name":"admission control", "command":[python(), "analyze.py"], "cwd":package,
                "requirements":{"runtime":"fixture", "source_fingerprint":"build-v1"},
            }))
            .unwrap()["analysis"]
            .clone();
        Self {
            _root: root,
            store,
            engine,
            analysis,
        }
    }

    fn request(&self) -> Value {
        json!({"run_id":"recording", "analysis_id":self.analysis["id"], "timeout_seconds":10})
    }

    async fn settle(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(10), self.engine.wait(id))
            .await
            .unwrap()
            .unwrap()
    }
}

/// An actual SQLite writer holds admission's document commit. The independent
/// thread has a fallback so a blocked event loop fails instead of deadlocking.
fn writer_lock(
    store: &Store,
    exclusive: bool,
) -> (mpsc::Sender<()>, std::thread::JoinHandle<bool>) {
    let database = store.root().join("hunin.sqlite");
    let (ready, started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let connection = rusqlite::Connection::open(database).unwrap();
        connection
            .execute_batch(if exclusive {
                "PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE"
            } else {
                "BEGIN IMMEDIATE"
            })
            .unwrap();
        ready.send(()).unwrap();
        let timely = released.recv_timeout(Duration::from_secs(2)).is_ok();
        connection.execute_batch("ROLLBACK").unwrap();
        timely
    });
    started.recv().unwrap();
    (release, thread)
}

#[tokio::test]
async fn blocking_admission_preserves_event_loop_progress_and_survives_dropped_caller() {
    let fixture = Fixture::new();
    let (release, writer) = writer_lock(&fixture.store, false);
    let engine = fixture.engine.clone();
    let request = fixture.request();
    let caller = tokio::spawn(async move { engine.run(&request).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    caller.abort();
    let _ = caller.await;
    let _ = release.send(());
    assert!(
        writer.join().unwrap(),
        "analysis admission blocked the async worker until the writer-lock fallback"
    );
    let id = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(row) = fixture.store.documents("analysis-results").unwrap().first() {
                break row["id"].as_str().unwrap().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("owned admission disappeared when its caller was dropped");
    fixture.engine.cancel(&id).await.unwrap();
    assert_eq!(fixture.settle(&id).await["status"], "cancelled");
    fixture.engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn capacity_rejection_is_pre_admission_and_permits_cover_terminal_cleanup() {
    let fixture = Fixture::new();
    let started = fixture.engine.run(&fixture.request()).await.unwrap();
    let error = fixture
        .engine
        .clone()
        .run(&fixture.request())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("capacity"), "{error:#}");
    assert_eq!(
        fixture.store.documents("analysis-results").unwrap().len(),
        1
    );
    let id = started["result_set_id"].as_str().unwrap();
    fixture.engine.cancel(id).await.unwrap();
    assert_eq!(fixture.settle(id).await["status"], "cancelled");
    let restarted = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match fixture.engine.run(&fixture.request()).await {
                Ok(started) => break started,
                Err(error) if error.to_string().contains("capacity") => {
                    tokio::task::yield_now().await
                }
                Err(error) => panic!("{error:#}"),
            }
        }
    })
    .await
    .expect("terminal attempt leaked its admission permit");
    fixture
        .engine
        .cancel(restarted["result_set_id"].as_str().unwrap())
        .await
        .unwrap();
    fixture.engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn preparation_failure_releases_capacity_without_admitting_an_attempt() {
    let fixture = Fixture::new();
    let mut run = fixture.store.run("recording").unwrap();
    run.source["fingerprint"] = json!("different-build");
    fixture.store.save_run(&run).unwrap();
    let error = fixture.engine.run(&fixture.request()).await.unwrap_err();
    assert!(error.to_string().contains("applicability mismatch"));
    assert!(
        fixture
            .store
            .documents("analysis-results")
            .unwrap()
            .is_empty()
    );
    run.source["fingerprint"] = json!("build-v1");
    fixture.store.save_run(&run).unwrap();
    let started = fixture.engine.run(&fixture.request()).await.unwrap();
    fixture
        .engine
        .cancel(started["result_set_id"].as_str().unwrap())
        .await
        .unwrap();
    fixture.engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn default_capacity_is_two_and_separate_engine_families_are_independent() {
    let fixture = Fixture::new();
    fixture.engine.run(&fixture.request()).await.unwrap();
    let other = Engine::new(fixture.store.clone());
    other.run(&fixture.request()).await.unwrap();
    other.clone().run(&fixture.request()).await.unwrap();
    let error = other.run(&fixture.request()).await.unwrap_err();
    assert!(error.to_string().contains("capacity"));
    assert_eq!(
        fixture.store.documents("analysis-results").unwrap().len(),
        3
    );
    fixture.engine.shutdown().await.unwrap();
    other.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_settles_preparation_and_cannot_leave_a_late_owned_attempt() {
    let fixture = Fixture::new();
    // EXCLUSIVE blocks the initial recovery read, before an attempt owns an
    // active-map entry. Shutdown must drain its preparation permit too.
    let (release, writer) = writer_lock(&fixture.store, true);
    let engine = fixture.engine.clone();
    let request = fixture.request();
    let caller = tokio::spawn(async move { engine.run(&request).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let engine = fixture.engine.clone();
    let mut shutdown = tokio::spawn(async move { engine.shutdown().await });
    tokio::select! {
        result = &mut shutdown => panic!("shutdown returned while preparation retained admission capacity: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(20)) => {},
    }
    let _ = release.send(());
    assert!(writer.join().unwrap());
    tokio::time::timeout(Duration::from_secs(10), shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let _ = caller.await.unwrap();
    let rows = fixture.store.documents("analysis-results").unwrap();
    assert!(
        rows.is_empty(),
        "preparation admitted after shutdown closed the engine: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| row["status"] != "running"),
        "{rows:?}"
    );
    assert!(
        fixture
            .engine
            .run(&fixture.request())
            .await
            .unwrap_err()
            .to_string()
            .contains("shutting down")
    );
}

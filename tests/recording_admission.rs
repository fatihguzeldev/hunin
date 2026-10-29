use hunin::{
    execution::{Engine, StartRequest},
    storage::Store,
};
use serde_json::json;
use std::{fs, path::PathBuf, sync::mpsc, time::Duration};

struct Fixture {
    root: tempfile::TempDir,
    store: Store,
    engine: Engine,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("store")).unwrap();
        let engine = Engine::with_admission_limit(store.clone(), 1).unwrap();
        fs::write(root.path().join("target.py"), "import pathlib, time\npathlib.Path('launched').write_text('yes')\nwhile not pathlib.Path('release').exists(): time.sleep(0.01)\npathlib.Path('exiting').write_text('yes')\n").unwrap();
        Self {
            root,
            store,
            engine,
        }
    }
    fn request(&self) -> StartRequest {
        serde_json::from_value(json!({"command":[if cfg!(windows){"python"}else{"python3"},"target.py"],"cwd":self.root.path(),"runtime":"none","timeout_seconds":20})).unwrap()
    }
    async fn file(&self, name: &str) {
        let path = self.root.path().join(name);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !path.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
}

fn writer_lock(store: &Store) -> (mpsc::Sender<()>, std::thread::JoinHandle<bool>) {
    let database: PathBuf = store.root().join("hunin.sqlite");
    let (ready, started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let connection = rusqlite::Connection::open(database).unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        ready.send(()).unwrap();
        let timely = released.recv_timeout(Duration::from_secs(2)).is_ok();
        connection.execute_batch("ROLLBACK").unwrap();
        timely
    });
    started.recv().unwrap();
    (release, thread)
}

#[tokio::test]
async fn recording_admission_is_responsive_and_survives_dropped_acknowledgement() {
    let fixture = Fixture::new();
    let (release, writer) = writer_lock(&fixture.store);
    let engine = fixture.engine.clone();
    let request = fixture.request();
    let caller = tokio::spawn(async move { engine.start(request).await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    caller.abort();
    let _ = caller.await;
    let _ = release.send(());
    assert!(
        writer.join().unwrap(),
        "recording admission blocked the async worker"
    );
    let id = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(run) = fixture.store.runs(1, 0).unwrap().first() {
                break run.id.clone();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    fixture.engine.cancel(&id).await.unwrap();
    assert_eq!(fixture.engine.wait(&id).await.unwrap().status, "canceled");
    fixture.engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn recording_capacity_is_preparation_to_terminal_and_shared_only_by_clones() {
    let fixture = Fixture::new();
    let first = fixture.engine.start(fixture.request()).await.unwrap();
    let mut invalid_preparation = fixture.request();
    invalid_preparation.cwd = Some(fixture.root.path().join("absent"));
    let error = fixture
        .engine
        .clone()
        .start(invalid_preparation)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("capacity"), "{error:#}");
    assert_eq!(fixture.store.runs(10, 0).unwrap().len(), 1);
    let other = Engine::with_admission_limit(fixture.store.clone(), 1).unwrap();
    other.start(fixture.request()).await.unwrap();
    fixture.engine.cancel(&first.id).await.unwrap();
    fixture.engine.shutdown().await.unwrap();
    other.shutdown().await.unwrap();
    assert!(
        fixture
            .store
            .runs(10, 0)
            .unwrap()
            .iter()
            .all(|run| run.status != "running")
    );
}

#[tokio::test]
async fn failed_preparation_and_invalid_duration_do_not_leak_recording_capacity() {
    let fixture = Fixture::new();
    let mut request = fixture.request();
    request.cwd = Some(fixture.root.path().join("absent"));
    assert!(fixture.engine.start(request).await.is_err());
    let mut request = fixture.request();
    request.timeout_seconds = Some(1e300);
    assert!(
        fixture
            .engine
            .start(request)
            .await
            .unwrap_err()
            .to_string()
            .contains("86400")
    );
    assert!(fixture.store.runs(10, 0).unwrap().is_empty());
    fixture.engine.start(fixture.request()).await.unwrap();
    fixture.engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_drains_pending_admission_and_cannot_launch_after_return() {
    let fixture = Fixture::new();
    let (release, writer) = writer_lock(&fixture.store);
    let engine = fixture.engine.clone();
    let request = fixture.request();
    let caller = tokio::spawn(async move { engine.start(request).await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    let engine = fixture.engine.clone();
    let mut shutdown = tokio::spawn(async move { engine.shutdown().await });
    tokio::select! {
        result=&mut shutdown=>panic!("shutdown abandoned an owned preparation: {result:?}"),
        _=tokio::time::sleep(Duration::from_millis(30))=>{},
    }
    let _ = release.send(());
    assert!(writer.join().unwrap());
    tokio::time::timeout(Duration::from_secs(10), shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let _ = caller.await.unwrap();
    assert!(!fixture.root.path().join("launched").exists());
    assert!(
        fixture
            .store
            .runs(10, 0)
            .unwrap()
            .iter()
            .all(|run| run.status != "running")
    );
    assert!(
        fixture
            .engine
            .start(fixture.request())
            .await
            .unwrap_err()
            .to_string()
            .contains("shutting down")
    );
}

#[tokio::test]
async fn terminal_retention_keeps_the_async_worker_responsive_and_holds_capacity() {
    let fixture = Fixture::new();
    let run = fixture.engine.start(fixture.request()).await.unwrap();
    fixture.file("launched").await;
    let (release, writer) = writer_lock(&fixture.store);
    fs::write(fixture.root.path().join("release"), "yes").unwrap();
    fixture.file("exiting").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        fixture
            .engine
            .start(fixture.request())
            .await
            .unwrap_err()
            .to_string()
            .contains("capacity")
    );
    let _ = release.send(());
    assert!(
        writer.join().unwrap(),
        "terminal retention blocked the async worker until the writer fallback"
    );
    assert_eq!(
        fixture.engine.wait(&run.id).await.unwrap().status,
        "completed"
    );
    fixture.engine.shutdown().await.unwrap();
}

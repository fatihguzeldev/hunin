use hunin::{model::runs::Run, operations::Operations, storage::Store};
use serde_json::json;
use std::{fs, sync::mpsc, time::Duration};

#[tokio::test]
async fn analyzer_registration_and_result_reads_keep_the_async_worker_responsive() {
    for (operation, exclusive) in [("analyses.register", false), ("analyses.results", true)] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("store")).unwrap();
        store
            .save_run(&Run {
                id: "recording".into(),
                name: "worker control".into(),
                status: "completed".into(),
                command: vec![],
                cwd: root.path().to_string_lossy().into_owned(),
                runtime: "fixture".into(),
                started_at_us: 1,
                finished_at_us: Some(2),
                exit_code: Some(0),
                diagnostics: vec![],
                source: json!({}),
                correctness: json!({}),
            })
            .unwrap();
        let package = root.path().join("package");
        fs::create_dir(&package).unwrap();
        fs::write(
            package.join("analyze.py"),
            "print('unused registration control')\n",
        )
        .unwrap();
        let operations = Operations::new(store.clone());
        let database = store.root().join("hunin.sqlite");
        let (ready, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let writer = std::thread::spawn(move || {
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
        let params = if operation == "analyses.register" {
            json!({"name":"worker control","command":[if cfg!(windows) {"python"} else {"python3"},"analyze.py"],"cwd":package,"requirements":{"runtime":"fixture","source_independent":true}})
        } else {
            json!({"run_id":"recording"})
        };
        let task = tokio::spawn(async move { operations.call(operation, params).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let _ = release.send(());
        assert!(
            writer.join().unwrap(),
            "{operation} blocked the async worker until the writer fallback"
        );
        task.await.unwrap().unwrap();
    }
}

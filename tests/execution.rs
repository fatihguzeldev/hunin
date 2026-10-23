use hunin::{
    execution::{Engine, StartRequest},
    storage::Store,
};
use serde_json::json;

#[tokio::test]
async fn records_real_exit_and_times_out_owned_process_group() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let engine = Engine::new(store.clone());
    #[cfg(unix)]
    let command: Vec<String> = vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf expected; exit 7".into(),
    ];
    #[cfg(windows)]
    let command: Vec<String> = vec![
        "cmd".into(),
        "/C".into(),
        "echo expected & exit /b 7".into(),
    ];
    let run = engine
        .start(
            serde_json::from_value::<StartRequest>(
                json!({"command":command,"runtime":"none","expected_exit_code":7}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let run = engine.wait(&run.id).await.unwrap();
    assert_eq!(run.status, "completed");
    assert_eq!(run.exit_code, Some(7));
    assert_eq!(run.correctness["passed"], true);
    assert!(run.source["supervised_elapsed_us"].as_u64().unwrap() > 0);
    assert_eq!(run.source["supervised_execution_scope"], "target_process");
    assert!(
        run.source["collector_settings_fingerprint"]
            .as_str()
            .is_some()
    );
    assert!(
        store
            .artifacts(&run.id)
            .unwrap()
            .iter()
            .any(|a| a.name == "stdout.txt")
    );
    #[cfg(unix)]
    {
        let run=engine.start(serde_json::from_value::<StartRequest>(json!({"command":["/bin/sh","-c","sleep 30"],"runtime":"none","timeout_seconds":0.05})).unwrap()).await.unwrap();
        let run = engine.wait(&run.id).await.unwrap();
        assert_eq!(run.status, "failed");
        assert!(run.diagnostics.iter().any(|s| s.contains("timeout")));
    }
}

async fn wait_file(path: &std::path::Path) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !path.is_file() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("controlled child did not start");
}

#[cfg(unix)]
struct ChildCleanup(std::process::Child);
#[cfg(unix)]
impl Drop for ChildCleanup {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_allows_graceful_cleanup_and_stops_owned_descendants() {
    let root = tempfile::tempdir().unwrap();
    let pid_file = root.path().join("child.pid");
    let stopped = root.path().join("stopped");
    let engine = Engine::new(Store::open(&root.path().join("data")).unwrap());
    let mut unrelated = ChildCleanup(
        std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap(),
    );
    let command = vec![
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        "trap 'echo graceful > \"$2\"; wait; exit 0' TERM; sleep 30 & echo $! > \"$1\"; wait"
            .to_owned(),
        "control".to_owned(),
        pid_file.display().to_string(),
        stopped.display().to_string(),
    ];
    let run = engine
        .start(
            serde_json::from_value(json!({"command":command,"runtime":"none","cwd":root.path()}))
                .unwrap(),
        )
        .await
        .unwrap();
    wait_file(&pid_file).await;
    let child_pid = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    engine.cancel(&run.id).await.unwrap();
    let completed = engine.wait(&run.id).await.unwrap();
    assert_eq!(completed.status, "canceled");
    assert_eq!(completed.source["target_outcome"]["status"], "canceled");
    assert!(
        stopped.is_file(),
        "TERM trap must run before forced cleanup"
    );
    let system = sysinfo::System::new_all();
    assert!(
        system
            .process(sysinfo::Pid::from_u32(child_pid))
            .is_none_or(|p| p.status() == sysinfo::ProcessStatus::Zombie),
        "owned descendant still running"
    );
    assert!(
        unrelated.0.try_wait().unwrap().is_none(),
        "unrelated process was killed"
    );
}

fn waiting_request(cwd: &std::path::Path) -> StartRequest {
    #[cfg(unix)]
    let command = json!(["/bin/sh", "-c", "sleep 30"]);
    #[cfg(windows)]
    let command = json!(["cmd.exe", "/C", "ping -n 31 127.0.0.1 >NUL"]);
    serde_json::from_value(json!({"command":command,"runtime":"none","cwd":cwd})).unwrap()
}

#[tokio::test]
async fn cancellation_crosses_engine_connections_but_shutdown_stays_with_its_owner() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let owner = Engine::new(store.clone());
    let run = owner.start(waiting_request(root.path())).await.unwrap();
    let observer = Engine::new(store.clone());
    assert_eq!(store.run(&run.id).unwrap().status, "running");
    observer.shutdown().await.unwrap();
    assert_eq!(
        store.run(&run.id).unwrap().status,
        "running",
        "observer shutdown must preserve the live owner's task"
    );
    observer.cancel(&run.id).await.unwrap();
    assert_eq!(owner.wait(&run.id).await.unwrap().status, "canceled");
    assert_eq!(
        store.document("cancellations", &run.id).unwrap()["run_id"],
        run.id
    );
    let second = owner.start(waiting_request(root.path())).await.unwrap();
    owner.shutdown().await.unwrap();
    assert_eq!(store.run(&second.id).unwrap().status, "canceled");
    assert!(owner.start(waiting_request(root.path())).await.is_err());
}

#[tokio::test]
async fn restart_recovers_only_proven_abandoned_owner() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let engine = Engine::new(store.clone());
    let live = engine.start(waiting_request(root.path())).await.unwrap();
    #[cfg(unix)]
    let mut exited = std::process::Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    #[cfg(windows)]
    let mut exited = std::process::Command::new("cmd.exe")
        .args(["/C", "exit /b 0"])
        .spawn()
        .unwrap();
    let pid = exited.id();
    exited.wait().unwrap();
    let mut stale = live.clone();
    stale.id = uuid::Uuid::new_v4().to_string();
    stale.source["owner"] =
        json!({"pid":pid,"process_start_time_seconds":null,"instance_id":"exited-owner"});
    store.save_run(&stale).unwrap();
    let _reopened = Engine::new(store.clone());
    assert_eq!(store.run(&stale.id).unwrap().status, "failed");
    assert_eq!(store.run(&live.id).unwrap().status, "running");
    engine.shutdown().await.unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn windows_job_stops_detached_descendant_after_launcher_exit() {
    let root = tempfile::tempdir().unwrap();
    let pid_file = root.path().join("child.pid");
    let engine = Engine::new(Store::open(&root.path().join("data")).unwrap());
    let script = format!(
        "$p=Start-Process -FilePath $env:ComSpec -ArgumentList '/C ping -n 31 127.0.0.1 >NUL' -PassThru; Set-Content -Encoding ascii -Path '{}' -Value $p.Id",
        pid_file.display().to_string().replace('\'', "''")
    );
    let run=engine.start(serde_json::from_value(json!({"command":["powershell.exe","-NoProfile","-NonInteractive","-Command",script],"runtime":"none","cwd":root.path()})).unwrap()).await.unwrap();
    let completed = engine.wait(&run.id).await.unwrap();
    assert_eq!(completed.status, "completed");
    wait_file(&pid_file).await;
    let pid = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    let system = sysinfo::System::new_all();
    assert!(
        system.process(sysinfo::Pid::from_u32(pid)).is_none(),
        "detached owned descendant escaped the Job"
    );
}

#![cfg(unix)]

use hunin::{model::runs::Run, storage::Store};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    io::Read,
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn initialized_rust_sdk_is_disabled_in_forked_child_without_losing_parent_records() {
    let directory = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "rust_sdk_fork_probe", "--nocapture"])
        .env("HUNIN_RUN_ID", "sdk-fork")
        .env("HUNIN_RECORD_DIR", directory.path().join("sdk"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            child.wait().unwrap();
            panic!("forked SDK probe entered inherited synchronization state");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(status.success(), "{stderr}");
    let records = std::fs::read_dir(directory.path().join("sdk"))
        .unwrap()
        .flat_map(|entry| {
            std::fs::read_to_string(entry.unwrap().path())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 3);
    assert_eq!(
        records
            .iter()
            .map(|record| &record["producer_id"])
            .collect::<HashSet<_>>()
            .len(),
        1
    );
    assert_eq!(
        records
            .iter()
            .map(|record| &record["pid"])
            .collect::<HashSet<_>>()
            .len(),
        1
    );
    let store = Store::open(&directory.path().join("store")).unwrap();
    store
        .save_run(&Run {
            id: "sdk-fork".into(),
            name: "fork control".into(),
            status: "completed".into(),
            command: vec!["fork-probe".into()],
            cwd: directory.path().to_string_lossy().into_owned(),
            runtime: "rust".into(),
            started_at_us: 0,
            finished_at_us: Some(9_007_199_254_740_991),
            exit_code: Some(0),
            diagnostics: vec![],
            source: json!({"fixture":"SDK fork lifecycle"}),
            correctness: json!({"passed":true}),
        })
        .unwrap();
    assert!(
        store
            .accept("sdk-fork", "fork-parent-records", &records)
            .unwrap()
    );
}

#[test]
#[ignore = "isolated subprocess entrypoint for initialized Rust SDK fork behavior"]
fn rust_sdk_fork_probe() {
    use hunin_sdk::{Context, enabled, event, flush, last_error};
    let context = Context::new("request");
    let parent = context.span("parent");
    assert!(enabled());
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        let mut unexpected = 0;
        if enabled() {
            unexpected |= 1;
        }
        if !Context::current_run().run_id.is_empty() {
            unexpected |= 2;
        }
        if !parent.context().run_id.is_empty() {
            unexpected |= 4;
        }
        let child = Context::new("child");
        if child.context_id.is_some() {
            unexpected |= 8;
        }
        if child.span("child-span").id.is_some() {
            unexpected |= 16;
        }
        if event(&child, "child-event", json!({})).is_some() {
            unexpected |= 32;
        }
        if flush().is_ok() || last_error().is_none() {
            unexpected |= 64;
        }
        drop(parent);
        // The test makes no general Rust/allocator safety claim after a
        // multithreaded fork; it exercises SDK state rejection before exec.
        unsafe { libc::_exit(unexpected) };
    }
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    assert!(libc::WIFEXITED(status));
    assert_eq!(
        libc::WEXITSTATUS(status),
        0,
        "child reused inherited SDK state"
    );
    parent.end();
    event(&context, "parent-after-fork", json!({}));
    flush().unwrap();
}

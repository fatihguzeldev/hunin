#![cfg(windows)]
use hunin_core::execution::PreparedLaunch;
use hunin_platforms::{
    inspect_process,
    native::control::{BorrowedProcess, OwnedProcess, OwnedScope, spawn_prepared},
};
use std::{
    collections::BTreeMap,
    io::Write,
    process::{Command, Stdio},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
};

struct ProcessHandle(HANDLE);
impl ProcessHandle {
    fn open(pid: u32) -> Self {
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!raw.is_null(), "{}", std::io::Error::last_os_error());
        Self(raw)
    }
    fn stopped(&self) -> bool {
        unsafe { WaitForSingleObject(self.0, 1000) == WAIT_OBJECT_0 }
    }
    fn running(&self) -> bool {
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_TIMEOUT }
    }
}
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn fixture(role: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "windows_lifetime_fixture",
            "--nocapture",
        ])
        .env("HUNIN_LIFETIME_ROLE", role)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}
#[test]
#[ignore = "invoked as a finite owned child by the Windows lifetime tests"]
fn windows_lifetime_fixture() {
    let Ok(role) = std::env::var("HUNIN_LIFETIME_ROLE") else {
        return;
    };
    if role == "child" {
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    let mut child = fixture("child").spawn().unwrap();
    std::fs::write(
        std::env::var_os("HUNIN_LIFETIME_PID").unwrap(),
        child.id().to_string(),
    )
    .unwrap();
    let token = std::env::var("HUNIN_CONTROL_TOKEN").unwrap();
    println!("HUNIN_READY {token}");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "HUNIN_GO");
    println!("HUNIN_WORK_COMPLETE {token}");
    std::io::stdout().flush().unwrap();
    line.clear();
    std::io::stdin().read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "HUNIN_EXIT");
    // The accepted Windows job owns cleanup of this still-running descendant.
    let _ = child.try_wait().unwrap();
}
#[test]
fn windows_discovery_retains_actual_parent_pid() {
    let mut child = OwnedProcess::spawn(&mut fixture("child"), OwnedScope::ProcessGroup).unwrap();
    assert_eq!(
        inspect_process(child.instance().pid).unwrap().parent_pid,
        Some(std::process::id())
    );
    let discovered = hunin_platforms::discover_processes().unwrap();
    let retained = discovered
        .processes
        .iter()
        .find(|p| p.same_instance(child.instance()))
        .unwrap();
    assert_eq!(retained.parent_pid, Some(std::process::id()));
    child
        .cancel(Duration::from_secs(1), Duration::from_secs(2))
        .unwrap();
}
#[test]
fn stopping_windows_borrowed_observation_preserves_target_and_unrelated_job() {
    let mut target = OwnedProcess::spawn(&mut fixture("child"), OwnedScope::ProcessGroup).unwrap();
    let mut unrelated =
        OwnedProcess::spawn(&mut fixture("child"), OwnedScope::ProcessGroup).unwrap();
    let target_handle = ProcessHandle::open(target.instance().pid);
    let unrelated_handle = ProcessHandle::open(unrelated.instance().pid);
    let mut borrowed = BorrowedProcess::attach(target.instance().clone()).unwrap();
    borrowed.snapshot().unwrap();
    borrowed.stop();
    assert!(borrowed.snapshot().is_err());
    assert!(target_handle.running());
    assert!(unrelated_handle.running());
    target
        .cancel(Duration::from_secs(1), Duration::from_secs(2))
        .unwrap();
    assert!(target_handle.stopped());
    assert!(unrelated_handle.running());
    unrelated
        .cancel(Duration::from_secs(1), Duration::from_secs(2))
        .unwrap();
}
#[test]
fn prepared_windows_job_reaps_descendant_after_its_leader_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let pidfile = directory.path().join("child.pid");
    let launch = PreparedLaunch {
        program: std::env::current_exe().unwrap(),
        args: [
            "--ignored",
            "--exact",
            "windows_lifetime_fixture",
            "--nocapture",
        ]
        .map(str::to_string)
        .into(),
        directory: directory.path().into(),
        environment: BTreeMap::from([
            ("HUNIN_CONTROL_PROTOCOL".into(), "nonce-lines-v1".into()),
            ("HUNIN_LIFETIME_ROLE".into(), "parent".into()),
            (
                "HUNIN_LIFETIME_PID".into(),
                pidfile.to_string_lossy().into_owned(),
            ),
        ]),
        stdout_limit: 4096,
    };
    let mut process = spawn_prepared(&launch, OwnedScope::ProcessGroup).unwrap();
    process.await_ready(Duration::from_secs(10)).unwrap();
    let pid: u32 = std::fs::read_to_string(pidfile).unwrap().parse().unwrap();
    let child = ProcessHandle::open(pid);
    assert!(child.running());
    process.release().unwrap();
    let start = std::time::Instant::now();
    while !process.work_complete().unwrap() {
        assert!(start.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(2));
    }
    process.finish().unwrap();
    let result = process.wait(Duration::from_secs(5)).unwrap();
    assert!(result.success);
    assert!(!result.cleanup_pending);
    assert!(child.stopped());
}

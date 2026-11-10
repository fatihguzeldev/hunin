#![cfg(unix)]
use hunin_platforms::{
    inspect_process,
    native::control::{BorrowedProcess, OwnedProcess, OwnedScope},
};
use std::{
    process::{Command, Stdio},
    time::Duration,
};

#[test]
fn stopping_borrowed_observation_never_stops_the_process() {
    let mut owned = OwnedProcess::spawn(
        Command::new("sh").args(["-c", "read gate; sleep 20"]),
        OwnedScope::ProcessGroup,
    )
    .unwrap();
    let mut borrowed = BorrowedProcess::attach(owned.instance().clone()).unwrap();
    borrowed.snapshot().unwrap();
    borrowed.stop();
    assert!(borrowed.snapshot().is_err());
    assert!(inspect_process(owned.instance().pid).is_ok());
    owned
        .cancel(Duration::from_millis(100), Duration::from_secs(2))
        .unwrap();
}

#[test]
fn owned_group_is_gated_and_cleaned_without_touching_an_unrelated_process() {
    let mut unrelated = OwnedProcess::spawn(
        Command::new("sh").args(["-c", "read gate; sleep 20"]),
        OwnedScope::ProcessGroup,
    )
    .unwrap();
    let mut target = OwnedProcess::spawn(
        Command::new("sh")
            .args(["-c", "read gate; printf released; sleep 20"])
            .stdout(Stdio::piped()),
        OwnedScope::ProcessGroup,
    )
    .unwrap();
    assert!(target.poll().unwrap().is_none());
    target.release(b"go\n").unwrap();
    assert!(target.release(b"go\n").is_err());
    target
        .cancel(Duration::from_millis(100), Duration::from_secs(2))
        .unwrap();
    assert!(unrelated.poll().unwrap().is_none());
    unrelated
        .cancel(Duration::from_millis(100), Duration::from_secs(2))
        .unwrap();
}

#[test]
fn bounded_tool_kills_descendants_when_its_leader_exits() {
    use std::{fs, time::Instant};
    let temporary = tempfile::tempdir().unwrap();
    let pidfile = temporary.path().join("child.pid");
    let mut command = Command::new("sh");
    command
        .args(["-c", "sleep 30 & echo $! > \"$1\"; exit 0", "probe"])
        .arg(&pidfile);
    hunin_platforms::tools::run(&mut command, temporary.path(), Duration::from_secs(2), 4096)
        .unwrap();
    let pid: u32 = fs::read_to_string(pidfile).unwrap().trim().parse().unwrap();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        if unsafe { libc::kill(pid as i32, 0) } != 0 {
            return;
        }
        #[cfg(target_os = "linux")]
        if std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .is_some_and(|s| {
                s.rsplit_once(')')
                    .is_some_and(|(_, tail)| tail.trim_start().starts_with('Z'))
            })
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    panic!("tool descendant survived leader exit");
}

#[test]
fn prepared_factory_uses_the_owned_group_for_core_cleanup() {
    use hunin_core::execution::PreparedLaunch;
    use hunin_platforms::native::control::spawn_prepared;
    let directory = tempfile::tempdir().unwrap();
    let launch = PreparedLaunch {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "printf 'HUNIN_READY\\n'; read gate; sleep 30 & printf '%s\\n' $!; exit 0".into(),
        ],
        directory: directory.path().into(),
        environment: Default::default(),
        stdout_limit: 1024,
    };
    let mut process = spawn_prepared(&launch, OwnedScope::ProcessGroup).unwrap();
    process.await_ready(Duration::from_secs(1)).unwrap();
    process.release().unwrap();
    let outcome = process.wait(Duration::from_secs(3)).unwrap();
    assert!(outcome.success);
    let pid: u32 = std::str::from_utf8(&outcome.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    #[cfg(target_os = "linux")]
    if std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .is_some_and(|s| {
            s.rsplit_once(')')
                .is_some_and(|(_, tail)| tail.trim_start().starts_with('Z'))
        })
    {
        return;
    }
    assert_ne!(
        unsafe { libc::kill(pid as i32, 0) },
        0,
        "owned descendant still executes after controller exit"
    );
}

#[test]
fn failed_tool_preserves_compiler_diagnostics_from_both_streams() {
    let temporary = tempfile::tempdir().unwrap();
    let output = hunin_platforms::tools::run(
        Command::new("sh").args([
            "-c",
            "printf stdout_diagnostic; printf stderr_diagnostic >&2; exit 2",
        ]),
        temporary.path(),
        Duration::from_secs(2),
        4096,
    )
    .unwrap();
    let error = hunin_platforms::tools::require_success(output).unwrap_err();
    assert!(error.message.contains("stdout_diagnostic"));
    assert!(error.message.contains("stderr_diagnostic"));
}

#[test]
fn prepared_owned_factory_uses_nonce_control_and_preserves_partial_stdout() {
    use hunin_core::execution::PreparedLaunch;
    use hunin_platforms::native::control::spawn_prepared;
    use std::{collections::BTreeMap, time::Instant};
    let launch = PreparedLaunch {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "printf 'preload'; printf 'HUNIN_READY %s\\r\\n' \"$HUNIN_CONTROL_TOKEN\"; read gate; test \"$gate\" = HUNIN_GO || exit 17; printf 'HUNIN_WORK_COMPLETE\\npartial'; sleep 0.12; printf 'HUNIN_WORK_COMPLETE %s\\n' \"$HUNIN_CONTROL_TOKEN\"; read gate; test \"$gate\" = HUNIN_EXIT".into()],
        directory: "/".into(),
        environment: BTreeMap::from([("HUNIN_CONTROL_PROTOCOL".into(), "nonce-lines-v1".into())]),
        stdout_limit: 1024,
    };
    let mut process = spawn_prepared(&launch, OwnedScope::ProcessGroup).unwrap();
    process.await_ready(Duration::from_secs(2)).unwrap();
    process.release().unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while !process.work_complete().unwrap() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(process.acknowledged_elapsed().unwrap() >= Duration::from_millis(90));
    process.finish().unwrap();
    let outcome = process.wait(Duration::from_secs(2)).unwrap();
    assert!(outcome.success);
    assert!(!outcome.cleanup_pending);
    assert_eq!(outcome.stdout, b"preloadHUNIN_WORK_COMPLETE\npartial");
}

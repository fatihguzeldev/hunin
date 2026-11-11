#![cfg(windows)]
use hunin_core::execution::ExecutionChild;
use hunin_platforms::{
    native::{
        Language,
        control::{OwnedProcess, OwnedScope},
        symbols::pe_pdb_identity,
        windows::{EtwRecording, resolve_pdb, resolve_pdb_profile},
    },
    tools,
};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
#[test]
#[ignore = "requires a pinned Windows MSVC developer environment and sampled-profile ETW permission"]
fn etw_is_ready_before_work_and_debug_artifacts_must_match() {
    let temporary = tempfile::tempdir().unwrap();
    let dir = temporary.path();
    let binary = dir.join("native.exe");
    let pdb = dir.join("native.pdb");
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native_windows.cpp");
    compile(&source, &binary, &pdb, dir, true);
    let identity = pe_pdb_identity(&binary).unwrap().0;
    let mut process = OwnedProcess::spawn(
        Command::new(&binary).stdout(Stdio::piped()),
        OwnedScope::ProcessGroup,
    )
    .unwrap();
    let mut stdout = BufReader::new(process.take_stdout().unwrap());
    let mut ready = String::new();
    stdout.read_line(&mut ready).unwrap();
    assert_eq!(ready.trim_end_matches(['\r', '\n']), "ready");
    let collector = EtwRecording::start(process.instance(), dir).unwrap();
    let recording_path = collector.path.clone();
    let mut input = ExecutionChild::take_stdin(&mut process).unwrap();
    input.write_all(b"p").unwrap();
    input.flush().unwrap();
    ready.clear();
    stdout.read_line(&mut ready).unwrap();
    assert_eq!(ready.trim_end_matches(['\r', '\n']), "probe-complete");
    let mut go_qpc = 0_i64;
    assert_ne!(
        unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut go_qpc) },
        0
    );
    input.write_all(b"g").unwrap();
    input.flush().unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let mut profile = collector.stop(Language::Cpp).unwrap();
    let index = profile
        .images
        .iter()
        .position(|image| image.identity == identity)
        .unwrap();
    let image = profile.images[index].clone();
    resolve_pdb_profile(&mut profile, &binary, &pdb).unwrap();
    let prework = profile
        .samples
        .iter()
        .filter(|sample| {
            sample.timestamp < go_qpc as u64
                && sample.frames.iter().any(|frame| {
                    frame
                        .function
                        .as_ref()
                        .is_some_and(|f| f.contains("prework_probe"))
                })
        })
        .count();
    assert!(
        prework > 0,
        "the configured source must capture independent pre-GO work"
    );
    assert!(
        !profile
            .samples
            .iter()
            .any(|sample| sample.timestamp < go_qpc as u64
                && sample.frames.iter().any(|frame| frame
                    .function
                    .as_ref()
                    .is_some_and(|f| f.contains("render_block"))))
    );
    eprintln!(
        "etw-prework-control {}",
        serde_json::json!({"before_go_samples":prework,"go_qpc":go_qpc,"frequency":profile.ticks_per_second,"readiness_basis":"synchronous owned StartTrace/TraceSetInformation acknowledgments; independent target prework probe recorded before measured release","profile_events_lost":profile.metadata["etw_events_lost"],"buffer_loss":profile.metadata["etw_log_buffers_lost"]})
    );
    assert!(profile.samples.iter().flat_map(|s| &s.frames).any(|frame| {
        frame
            .function
            .as_ref()
            .is_some_and(|f| f.contains("render_block"))
            && frame.source.as_ref().is_some_and(|source| {
                source.file.ends_with("native_windows.cpp") && source.line > 0
            })
    }));
    let mut wrong = image.clone();
    wrong.identity = "00000000-0000-0000-0000-000000000000:1".into();
    assert!(resolve_pdb(&wrong, &pdb, &mut profile.samples[0].frames).is_err());
    assert!(process.wait(Duration::from_secs(5)).unwrap().success());
    let truncated = dir.join("truncated.etl");
    std::fs::write(&truncated, &std::fs::read(&recording_path).unwrap()[..32]).unwrap();
    assert!(
        hunin_platforms::native::windows::import_etl(
            &truncated,
            process.instance(),
            Language::Cpp,
            profile.images.clone(),
            Duration::from_secs(1)
        )
        .is_err()
    );
    let excessive = dir.join("excessive.etl");
    std::fs::File::create(&excessive)
        .unwrap()
        .set_len(256 * 1024 * 1024 + 1)
        .unwrap();
    assert_eq!(
        hunin_platforms::native::windows::import_etl(
            &excessive,
            process.instance(),
            Language::Cpp,
            profile.images.clone(),
            Duration::from_secs(1)
        )
        .unwrap_err()
        .kind,
        hunin_platforms::process::ErrorKind::LimitExceeded
    );
    let held_pdb = dir.join("held.pdb");
    std::fs::rename(&pdb, &held_pdb).unwrap();
    assert!(resolve_pdb(&image, &pdb, &mut profile.samples[0].frames).is_err());
    std::fs::rename(&held_pdb, &pdb).unwrap();
    let wrong_source = dir.join("wrong.cpp");
    std::fs::write(
        &wrong_source,
        std::fs::read_to_string(&source)
            .unwrap()
            .replace("1000000", "1000001"),
    )
    .unwrap();
    let wrong_binary = dir.join("wrong.exe");
    let wrong_pdb = dir.join("wrong.pdb");
    compile(&wrong_source, &wrong_binary, &wrong_pdb, dir, true);
    assert_ne!(pe_pdb_identity(&wrong_binary).unwrap().0, identity);
    assert!(resolve_pdb(&image, &wrong_pdb, &mut profile.samples[0].frames).is_err());
    std::fs::rename(&pdb, &held_pdb).unwrap();
    std::fs::copy(&wrong_pdb, &pdb).unwrap();
    assert!(
        resolve_pdb(&image, &pdb, &mut profile.samples[0].frames).is_err(),
        "a matching filename must not substitute for the selected PDB build"
    );
    std::fs::remove_file(&pdb).unwrap();
    std::fs::rename(&held_pdb, &pdb).unwrap();
    let no_debug = dir.join("no-debug.exe");
    compile(&source, &no_debug, &dir.join("unused.pdb"), dir, false);
    assert!(pe_pdb_identity(&no_debug).is_err());
}

#[test]
#[ignore = "requires sampled-profile ETW permission and MSVC; stops only its own source before measured work"]
fn stopping_etw_before_go_preserves_the_borrowed_target() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path();
    let binary = directory.join("held.exe");
    let pdb = directory.join("held.pdb");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native_windows.cpp");
    compile(&source, &binary, &pdb, directory, true);
    let mut process = OwnedProcess::spawn(
        Command::new(&binary).stdout(Stdio::piped()),
        OwnedScope::ProcessGroup,
    )
    .unwrap();
    let mut stdout = BufReader::new(process.take_stdout().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "ready");
    let collector = EtwRecording::start(process.instance(), directory).unwrap();
    let mut input = ExecutionChild::take_stdin(&mut process).unwrap();
    input.write_all(b"p").unwrap();
    input.flush().unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "probe-complete");
    let mut profile = collector.stop(Language::Cpp).unwrap();
    resolve_pdb_profile(&mut profile, &binary, &pdb).unwrap();
    assert!(profile.samples.iter().any(|s| s.frames.iter().any(|f| {
        f.function
            .as_ref()
            .is_some_and(|name| name.contains("prework_probe"))
    })));
    assert!(!profile.samples.iter().any(|s| s.frames.iter().any(|f| {
        f.function
            .as_ref()
            .is_some_and(|name| name.contains("render_block"))
    })));
    assert!(process.poll().unwrap().is_none());
    hunin_platforms::read_resources(process.instance()).unwrap();
    process
        .cancel(Duration::from_secs(1), Duration::from_secs(2))
        .unwrap();
}

fn compile(source: &Path, binary: &Path, pdb: &Path, directory: &Path, debug: bool) {
    let mut command = Command::new("cl");
    command.args(["/nologo", "/Od", "/Oy-", "/std:c++17"]);
    if debug {
        command.arg("/Zi");
    }
    command
        .arg(source)
        .arg(format!("/Fo{}", binary.with_extension("obj").display()))
        .arg(format!("/Fe{}", binary.display()));
    if debug {
        command
            .args(["/link", "/DEBUG:FULL"])
            .arg(format!("/PDB:{}", pdb.display()));
    }
    tools::require_success(
        tools::run(
            &mut command,
            directory,
            Duration::from_secs(30),
            1024 * 1024,
        )
        .unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "requires Windows token APIs; disables privileges only in a separate owned test child"]
fn etw_permission_denial_is_reported_without_releasing_work() {
    let temporary = tempfile::tempdir().unwrap();
    let result = tools::run(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "etw_privilege_disabled_child",
                "--nocapture",
            ])
            .env("HUNIN_ETW_PRIVILEGE_CHILD", "1"),
        temporary.path(),
        Duration::from_secs(15),
        65536,
    )
    .unwrap();
    assert!(
        result.status.success(),
        "{} {}",
        result.stdout,
        result.stderr
    );
    assert!(
        result
            .stdout
            .contains("etw-denied-without-profile-privileges")
    );
    eprintln!("{}", result.stdout);
}

#[test]
#[ignore = "invoked only by the finite owned permission control"]
fn etw_privilege_disabled_child() {
    if std::env::var("HUNIN_ETW_PRIVILEGE_CHILD").as_deref() != Ok("1") {
        return;
    }
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        Security::{AdjustTokenPrivileges, TOKEN_ADJUST_PRIVILEGES, TOKEN_QUERY},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let mut token = std::ptr::null_mut();
    assert_ne!(
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            )
        },
        0
    );
    assert_ne!(
        unsafe {
            AdjustTokenPrivileges(
                token,
                1,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    unsafe {
        CloseHandle(token);
    }
    let area = tempfile::tempdir().unwrap();
    let target = hunin_platforms::inspect_process(std::process::id()).unwrap();
    hunin_platforms::read_resources(&target).unwrap();
    let error = match EtwRecording::start(&target, area.path()) {
        Ok(recording) => {
            drop(recording);
            panic!("privilege-disabled source unexpectedly started")
        }
        Err(error) => error,
    };
    assert_eq!(
        error.kind,
        hunin_platforms::process::ErrorKind::AccessDenied,
        "{error}"
    );
    println!("etw-denied-without-profile-privileges: {error}");
}

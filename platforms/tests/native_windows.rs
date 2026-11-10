#![cfg(windows)]
use hunin_platforms::{
    native::{
        Language,
        control::{OwnedProcess, OwnedScope},
        symbols::pe_pdb_identity,
        windows::{EtwRecording, resolve_pdb},
    },
    tools,
};
use std::{
    io::{BufRead, BufReader},
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
    let object = dir.join("native.obj");
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native_windows.cpp");
    tools::require_success(
        tools::run(
            Command::new("cl")
                .args(["/nologo", "/Zi", "/Od", "/Oy-", "/std:c++17"])
                .arg(source)
                .arg(format!("/Fo{}", object.display()))
                .arg(format!("/Fe{}", binary.display()))
                .args(["/link", "/DEBUG:FULL"])
                .arg(format!("/PDB:{}", pdb.display())),
            dir,
            Duration::from_secs(30),
            1024 * 1024,
        )
        .unwrap(),
    )
    .unwrap();
    let identity = pe_pdb_identity(&binary).unwrap().0;
    let mut process = OwnedProcess::spawn(
        Command::new(&binary).stdout(Stdio::piped()),
        OwnedScope::ProcessGroup,
    )
    .unwrap();
    let mut stdout = BufReader::new(process.take_stdout().unwrap());
    let mut ready = String::new();
    stdout.read_line(&mut ready).unwrap();
    assert_eq!(ready, "ready\n");
    let collector = EtwRecording::start(process.instance(), dir).unwrap();
    process.release(b"g\n").unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let mut profile = collector.stop(Language::Cpp).unwrap();
    let index = profile
        .images
        .iter()
        .position(|image| image.identity == identity)
        .unwrap();
    let image = profile.images[index].clone();
    for sample in &mut profile.samples {
        resolve_pdb(&image, &pdb, &mut sample.frames).unwrap();
    }
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
}

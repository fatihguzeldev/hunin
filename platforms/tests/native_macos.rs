#![cfg(target_os = "macos")]
use hunin_platforms::{
    native::{
        Language,
        control::{OwnedProcess, OwnedScope},
        macos::{parse_sample, parse_uuids, resolve_atos, resolve_profile, verify_mach_build},
    },
    process::ErrorKind,
    read_resources, tools,
};
use std::{
    io::{BufRead, BufReader, Read},
    process::{Command, Stdio},
    time::Duration,
};
#[test]
fn diagnostic_sample_resolves_only_matching_build_and_cpu_matches_getrusage() {
    let temporary = tempfile::tempdir().unwrap();
    let dir = temporary.path();
    let object = dir.join("native.o");
    let binary = dir.join("native");
    let debug = dir.join("native.dSYM");
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native.cpp");
    tools::require_success(
        tools::run(
            Command::new("clang++")
                .args([
                    "-O1",
                    "-g",
                    "-fno-inline",
                    "-fno-omit-frame-pointer",
                    "-std=c++17",
                    "-c",
                ])
                .arg(source)
                .arg("-o")
                .arg(&object),
            dir,
            Duration::from_secs(20),
            65536,
        )
        .unwrap(),
    )
    .unwrap();
    tools::require_success(
        tools::run(
            Command::new("clang++").arg(&object).arg("-o").arg(&binary),
            dir,
            Duration::from_secs(20),
            65536,
        )
        .unwrap(),
    )
    .unwrap();
    tools::require_success(
        tools::run(
            Command::new("dsymutil").arg(&binary).arg("-o").arg(&debug),
            dir,
            Duration::from_secs(20),
            65536,
        )
        .unwrap(),
    )
    .unwrap();
    let uuid = tools::require_success(
        tools::run(
            Command::new("xcrun")
                .args(["dwarfdump", "--uuid"])
                .arg(&binary),
            dir,
            Duration::from_secs(10),
            65536,
        )
        .unwrap(),
    )
    .unwrap();
    let uuid = parse_uuids(&uuid.stdout).unwrap()[0].0.clone();
    let mut process = OwnedProcess::spawn(
        Command::new(&binary)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
        OwnedScope::ProcessGroup,
    )
    .unwrap();
    let mut stdout = BufReader::new(process.take_stdout().unwrap());
    let mut ready = String::new();
    stdout.read_line(&mut ready).unwrap();
    assert_eq!(ready, "ready\n");
    let before = read_resources(process.instance()).unwrap();
    process.release(b"g\n").unwrap();
    let report = dir.join("sample.txt");
    tools::require_success(
        tools::run(
            Command::new("sample")
                .arg(process.instance().pid.to_string())
                .args(["1", "1", "-mayDie", "-fullPaths", "-file"])
                .arg(&report),
            dir,
            Duration::from_secs(10),
            65536,
        )
        .unwrap(),
    )
    .unwrap();
    let during = read_resources(process.instance()).unwrap();
    let mut profile =
        parse_sample(&std::fs::read_to_string(&report).unwrap(), Language::Cpp).unwrap();
    resolve_profile(&mut profile, &binary, &debug, &uuid, dir).unwrap();
    assert!(profile.aggregates.iter().any(|n| {
        n.frame
            .function
            .as_ref()
            .is_some_and(|f| f.contains("render_block"))
            && n.frame
                .source
                .as_ref()
                .is_some_and(|s| s.file.ends_with("native.cpp") && s.line > 0)
    }));
    assert_eq!(
        verify_mach_build(
            &binary,
            &debug,
            "00000000-0000-0000-0000-000000000000",
            "arm64",
            dir
        )
        .unwrap_err()
        .kind,
        ErrorKind::SymbolMismatch
    );
    let stripped = dir.join("stripped");
    tools::require_success(
        tools::run(
            Command::new("strip")
                .args(["-S", "-o"])
                .arg(&stripped)
                .arg(&binary),
            dir,
            Duration::from_secs(10),
            65536,
        )
        .unwrap(),
    )
    .unwrap();
    std::fs::remove_dir_all(&debug).unwrap();
    std::fs::remove_file(&object).unwrap();
    let node = profile
        .aggregates
        .iter()
        .find(|n| {
            n.frame
                .function
                .as_ref()
                .is_some_and(|f| f.contains("render_block"))
        })
        .unwrap();
    let image = &profile.images[node.frame.image.unwrap()];
    let no_debug = resolve_atos(image, &binary, &stripped, &[node.frame.address], dir).unwrap();
    assert!(no_debug[0].source.is_none());
    let cpu_delta = during.user_cpu.nanoseconds().unwrap()
        + during.system_cpu.nanoseconds().unwrap()
        - before.user_cpu.nanoseconds().unwrap()
        - before.system_cpu.nanoseconds().unwrap();
    assert!(cpu_delta > 100_000_000);
    assert!(process.wait(Duration::from_secs(6)).unwrap().success());
    let mut output = String::new();
    stdout.read_to_string(&mut output).unwrap();
    let values: Vec<u64> = output
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    assert_eq!(values.len(), 3);
    assert!(values[0] > 0 && values[1] != 0);
    assert!(values[2] > 1_000_000);
    assert!(cpu_delta < (values[2] + 100_000) * 1000);
    // sample has no proven recording-ready handshake; this is a diagnostic importer test.
}

#[test]
fn c_and_rust_share_collection_but_resolve_their_own_builds_and_source() {
    for (language, extension) in [(Language::C, "c"), (Language::Rust, "rs")] {
        let temporary = tempfile::tempdir().unwrap();
        let dir = temporary.path();
        let binary = dir.join("workload");
        let debug = dir.join("workload.dSYM");
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/native.{extension}"));
        if language == Language::C {
            let object = dir.join("native.o");
            tools::require_success(
                tools::run(
                    Command::new("clang")
                        .args(["-O1", "-g", "-fno-inline", "-fno-omit-frame-pointer", "-c"])
                        .arg(&source)
                        .arg("-o")
                        .arg(&object),
                    dir,
                    Duration::from_secs(20),
                    65536,
                )
                .unwrap(),
            )
            .unwrap();
            tools::require_success(
                tools::run(
                    Command::new("clang").arg(&object).arg("-o").arg(&binary),
                    dir,
                    Duration::from_secs(20),
                    65536,
                )
                .unwrap(),
            )
            .unwrap();
        } else {
            tools::require_success(
                tools::run(
                    Command::new("rustc")
                        .args([
                            "-g",
                            "-C",
                            "opt-level=1",
                            "-C",
                            "force-frame-pointers=yes",
                            "-C",
                            "save-temps=yes",
                        ])
                        .arg(&source)
                        .arg("-o")
                        .arg(&binary),
                    dir,
                    Duration::from_secs(20),
                    65536,
                )
                .unwrap(),
            )
            .unwrap();
        }
        tools::require_success(
            tools::run(
                Command::new("dsymutil").arg(&binary).arg("-o").arg(&debug),
                dir,
                Duration::from_secs(20),
                65536,
            )
            .unwrap(),
        )
        .unwrap();
        let uuid = tools::require_success(
            tools::run(
                Command::new("xcrun")
                    .args(["dwarfdump", "--uuid"])
                    .arg(&binary),
                dir,
                Duration::from_secs(10),
                65536,
            )
            .unwrap(),
        )
        .unwrap();
        let uuid = parse_uuids(&uuid.stdout).unwrap()[0].0.clone();
        let mut process = OwnedProcess::spawn(
            Command::new(&binary).stdout(Stdio::piped()),
            OwnedScope::ProcessGroup,
        )
        .unwrap();
        let mut stdout = BufReader::new(process.take_stdout().unwrap());
        let mut ready = String::new();
        stdout.read_line(&mut ready).unwrap();
        assert_eq!(ready, "ready\n");
        process.release(b"g\n").unwrap();
        let report = dir.join("sample.txt");
        tools::require_success(
            tools::run(
                Command::new("sample")
                    .arg(process.instance().pid.to_string())
                    .args(["1", "1", "-mayDie", "-fullPaths", "-file"])
                    .arg(&report),
                dir,
                Duration::from_secs(10),
                65536,
            )
            .unwrap(),
        )
        .unwrap();
        let mut profile =
            parse_sample(&std::fs::read_to_string(report).unwrap(), language).unwrap();
        resolve_profile(&mut profile, &binary, &debug, &uuid, dir).unwrap();
        assert!(
            profile.aggregates.iter().any(|n| n
                .frame
                .function
                .as_ref()
                .is_some_and(|f| f.contains("render_block"))
                && n.frame
                    .source
                    .as_ref()
                    .is_some_and(
                        |s| s.file.ends_with(&format!("native.{extension}")) && s.line > 0
                    )),
            "{language:?} source resolution missing: {:?}",
            profile
                .aggregates
                .iter()
                .filter(|n| n
                    .frame
                    .function
                    .as_ref()
                    .is_some_and(|f| f.contains("render_block")))
                .map(|n| &n.frame)
                .collect::<Vec<_>>()
        );
        assert!(process.wait(Duration::from_secs(5)).unwrap().success());
    }
}

use hunin_core::execution::PreparedProcess;
use hunin_runtimes::{Limits, RuntimeKind, prepare};
use std::{fs, path::PathBuf, process::Command, time::Duration};
#[test]
#[ignore = "requires selected full-JDK HUNIN_JAVA"]
fn jfr_missing_debug_and_mismatched_class_controls_keep_methods_without_false_source_binding() {
    let executable = PathBuf::from(std::env::var("HUNIN_JAVA").unwrap());
    let compiler = executable.with_file_name(if cfg!(windows) { "javac.exe" } else { "javac" });
    let original = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Preview.java");
    for no_debug in [false, true] {
        let area = tempfile::tempdir().unwrap();
        let prepared = prepare(RuntimeKind::HotSpot, &executable, &original, area.path()).unwrap();
        let replacement = area.path().join("replacement");
        fs::create_dir(&replacement).unwrap();
        let source = replacement.join("Preview.java");
        let text = fs::read_to_string(&original).unwrap();
        fs::write(
            &source,
            if no_debug {
                text
            } else {
                format!("\n\n{text}")
            },
        )
        .unwrap();
        let mut command = Command::new(&compiler);
        command
            .arg(if no_debug { "-g:none" } else { "-g" })
            .args(["-proc:none", "-d"])
            .arg(area.path())
            .arg(source);
        hunin_platforms::tools::require_success(
            hunin_platforms::tools::run(
                &mut command,
                area.path(),
                Duration::from_secs(30),
                64 * 1024,
            )
            .unwrap(),
        )
        .unwrap();
        let mut process = PreparedProcess::spawn(&prepared.launch).unwrap();
        process.await_ready(Duration::from_secs(10)).unwrap();
        process.release().unwrap();
        assert!(process.wait(Duration::from_secs(30)).unwrap().success);
        let extraction = prepared.extract("jvm", Limits::default()).unwrap();
        let frames = extraction
            .records
            .iter()
            .filter(|r| r.schema == "jfr.frame" && r.function.as_deref() == Some("Preview.resize"))
            .collect::<Vec<_>>();
        assert!(!frames.is_empty());
        for frame in frames {
            assert!(frame.location.is_none());
            assert!(frame.payload["source"].is_null());
            assert_ne!(
                frame.payload["class_file_sha256"],
                frame.payload["expected_class_file_sha256"]
            );
            assert!(frame.payload["descriptor"].is_string());
            if no_debug {
                assert!(frame.payload["line"].as_i64().unwrap() < 0);
            } else {
                assert!(frame.payload["line"].as_i64().unwrap() > 0);
            }
        }
    }
}

#[cfg(unix)]
#[test]
#[ignore = "requires selected full-JDK HUNIN_JAVA"]
fn jfr_compiler_manifest_hashes_the_compiled_snapshot_when_the_live_source_changes() {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let java = PathBuf::from(std::env::var("HUNIN_JAVA").unwrap());
    let root = tempfile::tempdir().unwrap();
    let tools = root.path().join("tools");
    let area = root.path().join("capture");
    fs::create_dir(&tools).unwrap();
    fs::create_dir(&area).unwrap();
    let source = root.path().join("Mutable.java");
    let replacement = root.path().join("changed.java");
    let original = b"public class Mutable {public static Object run(){return 1;}}\n";
    let changed = b"public class Mutable {public static Object run(){return 2;}}\n";
    fs::write(&source, original).unwrap();
    fs::write(&replacement, changed).unwrap();
    let quote =
        |path: &std::path::Path| format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"));
    let wrapper = tools.join("java");
    fs::write(
        &wrapper,
        format!("#!/bin/sh\nexec {} \"$@\"\n", quote(&java)),
    )
    .unwrap();
    let javac = tools.join("javac");
    fs::write(&javac,format!("#!/bin/sh\n{} \"$@\"\nresult=$?\nif [ \"$result\" -eq 0 ]; then cp {} {}; fi\nexit \"$result\"\n",quote(&java.with_file_name("javac")),quote(&replacement),quote(&source))).unwrap();
    for path in [&wrapper, &javac] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    prepare(RuntimeKind::HotSpot, &wrapper, &source, &area).unwrap();
    assert_eq!(
        fs::read(&source).unwrap(),
        changed,
        "the control must change the live file before manifest publication"
    );
    let manifest = fs::read_to_string(area.join("class-bindings.txt")).unwrap();
    assert!(
        manifest
            .lines()
            .any(|line| line == format!("source {:x}", Sha256::digest(original))),
        "manifest must describe the bytes javac received: {manifest}"
    );
}

#[test]
#[ignore = "requires selected full-JDK HUNIN_JAVA"]
fn real_jfr_stack_depth_limit_is_incomplete_while_retaining_observed_frames() {
    let java = PathBuf::from(std::env::var("HUNIN_JAVA").unwrap());
    let root = tempfile::tempdir().unwrap();
    let area = root.path().join("capture");
    fs::create_dir(&area).unwrap();
    let source = root.path().join("Deep.java");
    fs::write(&source,"public class Deep {static volatile long sink;static void descend(int depth){if(depth>0){descend(depth-1);return;}long value=0;for(long i=0;i<150000000;i++)value=(value*33)^i;sink=value;}public static Object run(){descend(32);return 32;}}\n").unwrap();
    let mut prepared = prepare(RuntimeKind::HotSpot, &java, &source, &area).unwrap();
    let flags = prepared
        .launch
        .args
        .iter_mut()
        .find(|arg| arg.starts_with("-XX:FlightRecorderOptions="))
        .unwrap();
    flags.push_str(",stackdepth=8");
    let mut process = PreparedProcess::spawn(&prepared.launch).unwrap();
    process.await_ready(Duration::from_secs(10)).unwrap();
    process.release().unwrap();
    assert!(process.wait(Duration::from_secs(20)).unwrap().success);
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(&prepared.report_path).unwrap()).unwrap();
    assert_eq!(report["output"], 32);
    let extracted = prepared.extract("jvm", Limits::default()).unwrap();
    assert!(!extracted.capture_complete);
    assert!(
        extracted
            .records
            .iter()
            .any(|r| r.schema == "jfr.sample" && r.payload["truncated"] == true)
    );
    assert!(
        extracted
            .records
            .iter()
            .any(|r| r.schema == "jfr.frame" && r.function.as_deref() == Some("Deep.descend"))
    );
    assert!(
        extracted
            .warnings
            .iter()
            .any(|w| w.contains("truncated sample stacks"))
    );
}

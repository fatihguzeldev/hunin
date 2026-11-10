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

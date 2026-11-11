#![cfg(target_os = "linux")]
use hunin_platforms::{
    native::{
        Language,
        control::{OwnedProcess, OwnedScope},
        perf::PerfCollector,
        symbols::resolve_elf_profile,
    },
    process::ErrorKind,
    tools,
};
use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[test]
#[ignore = "requires an explicitly provisioned perf_event_open environment; denial is not native baseline success"]
fn perf_is_armed_before_work_and_resolves_real_matched_code() {
    for (language, extension) in [
        (Language::C, "c"),
        (Language::Cpp, "cpp"),
        (Language::Rust, "rs"),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let dir = temporary.path();
        let binary = dir.join("native");
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/native.{extension}"));
        let mut command = Command::new(if language == Language::Rust {
            "rustc"
        } else if language == Language::C {
            "clang"
        } else {
            "clang++"
        });
        if language == Language::Rust {
            command.args([
                "-g",
                "-C",
                "opt-level=1",
                "-C",
                "force-frame-pointers=yes",
                "-C",
                "link-arg=-Wl,--build-id=sha1",
            ]);
        } else {
            command.args([
                "-O1",
                "-g",
                "-fno-inline",
                "-fno-omit-frame-pointer",
                "-Wl,--build-id=sha1",
            ]);
            if language == Language::Cpp {
                command.arg("-std=c++17");
            }
        }
        tools::require_success(
            tools::run(
                command.arg(source).arg("-o").arg(&binary),
                dir,
                Duration::from_secs(20),
                65536,
            )
            .unwrap(),
        )
        .unwrap();
        let mut process = OwnedProcess::spawn(
            Command::new(&binary).stdout(Stdio::piped()),
            OwnedScope::ProcessGroup,
        )
        .unwrap();
        let mut stdout = BufReader::new(process.take_stdout().unwrap());
        let mut ready = String::new();
        stdout.read_line(&mut ready).unwrap();
        assert_eq!(ready, "ready\n");
        let mut collector =
            PerfCollector::start(process.instance(), language, 99, 100000, 128).unwrap();
        process.release(b"g\n").unwrap();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(50));
            collector.drain().unwrap();
        }
        let mut profile = collector.stop().unwrap();
        assert!(!profile.samples.is_empty());
        let captured = profile.clone();
        let index = profile
            .images
            .iter()
            .position(|image| image.path == binary)
            .unwrap();
        assert!(!profile.images[index].identity.is_empty());
        resolve_elf_profile(&mut profile, &binary, dir).unwrap();
        assert!(profile.samples.iter().flat_map(|s| &s.frames).any(|frame| {
            frame
                .function
                .as_ref()
                .is_some_and(|f| f.contains("render_block"))
                && frame
                    .source
                    .as_ref()
                    .is_some_and(|s| s.file.ends_with(&format!("native.{extension}")) && s.line > 0)
        }));
        let stripped = dir.join("native-without-debug");
        std::fs::copy(&binary, &stripped).unwrap();
        tools::require_success(
            tools::run(
                Command::new("llvm-strip")
                    .arg("--strip-debug")
                    .arg(&stripped),
                dir,
                Duration::from_secs(10),
                65536,
            )
            .unwrap(),
        )
        .unwrap();
        let mut absent_debug = captured.clone();
        resolve_elf_profile(&mut absent_debug, &stripped, dir).unwrap();
        let justified_names: Vec<_> = absent_debug
            .samples
            .iter()
            .flat_map(|sample| &sample.frames)
            .filter(|frame| {
                frame
                    .function
                    .as_ref()
                    .is_some_and(|name| name.contains("render_block"))
            })
            .collect();
        assert!(
            !justified_names.is_empty(),
            "stripping debug must preserve remaining symbol-table names"
        );
        assert!(
            justified_names.iter().all(|frame| frame.source.is_none()),
            "removed debug information cannot retain source locations"
        );

        let wrong_source = dir.join("wrong.c");
        std::fs::write(&wrong_source,"__attribute__((noinline)) unsigned long render_block(unsigned long n){return n+1;} int main(void){return (int)render_block(8);}").unwrap();
        let wrong_binary = dir.join("native-wrong-build");
        tools::require_success(
            tools::run(
                Command::new("clang")
                    .args(["-g", "-O1", "-Wl,--build-id=sha1"])
                    .arg(&wrong_source)
                    .arg("-o")
                    .arg(&wrong_binary),
                dir,
                Duration::from_secs(20),
                65536,
            )
            .unwrap(),
        )
        .unwrap();
        let mut wrong_build = captured.clone();
        assert_eq!(
            resolve_elf_profile(&mut wrong_build, &wrong_binary, dir)
                .unwrap_err()
                .kind,
            ErrorKind::SymbolMismatch
        );
        assert_eq!(
            serde_json::to_value(&wrong_build.samples).unwrap(),
            serde_json::to_value(&captured.samples).unwrap(),
            "rejected symbols must leave captured frames unchanged"
        );
        assert!(process.wait(Duration::from_secs(5)).unwrap().success());
        let project = hunin_core::project::Project::create(dir, "native-storage").unwrap();
        let investigation = project
            .create_investigation("i", "native", serde_json::json!({}))
            .unwrap();
        let run = project
            .create_run(&investigation, "r", serde_json::json!({}))
            .unwrap();
        let target = project
            .register_target(
                &run,
                "t",
                serde_json::to_value(process.instance()).unwrap(),
                true,
            )
            .unwrap();
        let source = project
            .register_source(&run, profile.definition("code", target))
            .unwrap();
        let records = profile.records("t").unwrap();
        for chunk in records.chunks(128) {
            source.append(chunk).unwrap();
        }
        let expected = records
            .iter()
            .find(|record| {
                record
                    .function
                    .as_ref()
                    .is_some_and(|f| f.contains("render_block"))
            })
            .unwrap()
            .clone();
        let reference = source.record_ref(&expected.id);
        drop(source);
        drop(project);
        std::fs::remove_file(&binary).unwrap();
        let reopened = hunin_core::project::Project::open(dir, "native-storage").unwrap();
        assert_eq!(reopened.record(&reference, None).unwrap(), expected);

        assert!(
            profile
                .samples
                .iter()
                .all(|sample| sample.timestamp >= profile.start.unwrap())
        );
    }
}

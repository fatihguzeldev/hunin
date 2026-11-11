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
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires actual perf access and a controlled pre-arm executable mapping replacement"]
fn mapping_replaced_before_arm_cannot_borrow_the_initial_images_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let dir = temporary.path();
    let mut libraries = Vec::new();
    let mut symbols = Vec::new();
    for name in ["a", "b"] {
        let source = dir.join(format!("{name}.c"));
        let symbol = format!("mapping_{name}_work");
        std::fs::write(&source, format!("__attribute__((noinline)) unsigned long {symbol}(unsigned long n){{unsigned long value=1;while(n--){{value=value*1664525+1013904223;__asm__ volatile(\"\" : \"+r\"(value));}}return value;}}")).unwrap();
        let binary = dir.join(format!("lib{name}.so"));
        tools::require_success(
            tools::run(
                Command::new("clang")
                    .args([
                        "-shared",
                        "-fPIC",
                        "-O1",
                        "-g",
                        "-fno-omit-frame-pointer",
                        "-Wl,--build-id=sha1",
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
        let names = tools::require_success(
            tools::run(
                Command::new("llvm-nm").arg("--defined-only").arg(&binary),
                dir,
                Duration::from_secs(10),
                65536,
            )
            .unwrap(),
        )
        .unwrap()
        .stdout;
        symbols.push(
            names
                .lines()
                .find(|line| line.ends_with(&symbol))
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .to_owned(),
        );
        libraries.push(binary);
    }
    assert_eq!(
        symbols[0], symbols[1],
        "both functions must occupy the same ELF address"
    );
    let fixture = dir.join("replace");
    tools::require_success(
        tools::run(
            Command::new("clang")
                .args(["-O1", "-g", "-pthread"])
                .arg(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/perf-map-replacement.c"),
                )
                .args(["-ldl", "-o"])
                .arg(&fixture),
            dir,
            Duration::from_secs(20),
            65536,
        )
        .unwrap(),
    )
    .unwrap();
    let mut child = Command::new(&fixture)
        .args(&libraries)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut child = Child(child);
    let mut ready = String::new();
    output.read_line(&mut ready).unwrap();
    let words: Vec<_> = ready.split_whitespace().collect();
    assert_eq!(words[0], "ready");
    let begin = u64::from_str_radix(words[1], 16).unwrap();
    let end = u64::from_str_radix(words[2], 16).unwrap();
    let target = hunin_platforms::inspect_process(child.0.id()).unwrap();
    let switched = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(Instant::now() < deadline, "no perf descriptor appeared");
            if std::fs::read_dir("/proc/self/fd")
                .unwrap()
                .filter_map(std::result::Result::ok)
                .any(|entry| {
                    std::fs::read_link(entry.path())
                        .is_ok_and(|path| path.to_string_lossy().contains("perf_event"))
                })
            {
                break;
            }
            std::thread::sleep(Duration::from_micros(50));
        }
        input.write_all(b"s\n").unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let timestamp: u64 = line
            .strip_prefix("switched ")
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        (input, output, timestamp)
    });
    let collector = PerfCollector::start(&target, Language::C, 199, 100000, 128);
    let (mut input, mut output, replacement_ns) = switched.join().unwrap();
    let collector = collector.unwrap();
    input.write_all(b"g\n").unwrap();
    input.flush().unwrap();
    let mut done = String::new();
    output.read_line(&mut done).unwrap();
    assert!(done.starts_with("done "));
    let mut profile = collector.stop().unwrap();
    assert!(child.0.wait().unwrap().success());
    assert!(
        replacement_ns < profile.start.unwrap(),
        "the independent control must replace the mapping before perf is enabled"
    );
    assert!(
        profile
            .samples
            .iter()
            .any(|sample| begin <= sample.instruction_pointer && sample.instruction_pointer < end)
    );
    if profile
        .images
        .iter()
        .any(|image| image.path == libraries[0])
    {
        resolve_elf_profile(&mut profile, &libraries[0], dir).unwrap();
    }
    if profile
        .images
        .iter()
        .any(|image| image.path == libraries[1])
    {
        resolve_elf_profile(&mut profile, &libraries[1], dir).unwrap();
    }
    if let Some(destination) = std::env::var_os("HUNIN_PERF_REPLACEMENT_OUTPUT") {
        std::fs::write(destination, serde_json::to_vec_pretty(&serde_json::json!({"replacement_ns":replacement_ns,"begin":begin,"end":end,"profile":profile})).unwrap()).unwrap();
    }
    assert!(
        profile
            .samples
            .iter()
            .flat_map(|sample| &sample.frames)
            .all(|frame| frame.function.as_deref() != Some("mapping_a_work")),
        "replacement B executed; pre-arm A debug names cannot be attributed to its addresses"
    );
}
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

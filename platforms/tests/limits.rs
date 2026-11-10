#[cfg(unix)]
use hunin_platforms::native::control::{OwnedScope, spawn_prepared};
#[cfg(unix)]
use std::collections::BTreeMap;
#[allow(dead_code)]
mod support;
use hunin_core::{
    project::Project,
    query::{Query, QueryLimits},
};
use hunin_platforms::{
    inspect_process,
    native::{Language, coverage::parse_export},
    process::ErrorKind,
    read_resources,
};
use serde_json::json;
use std::time::{Duration, Instant};

#[test]
fn foreign_clock_and_oversized_extraction_fail_without_erasing_durable_evidence() {
    let root = tempfile::tempdir().unwrap();
    let project = Project::create(root.path(), "limits").unwrap();
    let investigation = project
        .create_investigation("i", "limits", json!({}))
        .unwrap();
    let run = project.create_run(&investigation, "r", json!({})).unwrap();
    let instance = inspect_process(std::process::id()).unwrap();
    let target = project
        .register_target(
            &run,
            "target",
            serde_json::to_value(&instance).unwrap(),
            false,
        )
        .unwrap();
    let first = read_resources(&instance).unwrap();
    let mut invalid =
        hunin_platforms::recording::resource_definition("invalid", target.clone(), &first);
    invalid.clocks[0].ticks_per_second = 0;
    assert!(project.register_source(&run, invalid).is_err());
    let source = project
        .register_source(
            &run,
            hunin_platforms::recording::resource_definition("resources", target, &first),
        )
        .unwrap();
    let known = first.to_record("known", "target").unwrap();
    source.append(std::slice::from_ref(&known)).unwrap();
    source.ready(json!({})).unwrap();
    let mut wrong = known.clone();
    wrong.id = "wrong-clock".into();
    wrong.time.as_mut().unwrap().clock = "foreign-clock".into();
    assert!(source.append(&[wrong]).is_err());
    let too_large = " ".repeat(64 * 1024 * 1024 + 1);
    let started = Instant::now();
    assert_eq!(
        parse_export(&too_large, &"a".repeat(64), Language::C, "after", json!({}))
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(too_large);
    source.report_gap("rejected-source-input",json!({"reason":"wrong clock and oversized source extraction rejected","complete":false})).unwrap();
    source.close(json!({"complete":false})).unwrap();
    let reference = source.record_ref("known");
    drop(source);
    drop(project);
    let reopened = Project::open(root.path(), "limits").unwrap();
    assert_eq!(reopened.record(&reference, None).unwrap(), known);
    assert_eq!(
        reopened
            .query(&Query::new(run), QueryLimits::default(), None)
            .unwrap()
            .records
            .len(),
        1
    );
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "uses selected Clang/LLVM toolchain for real raw recording and wrong-build controls"]
fn real_coverage_recording_size_and_build_guards_are_finite() {
    use hunin_platforms::native::coverage::CoverageBuild;
    let root = tempfile::tempdir().unwrap();
    let build = root.path().join("build");
    let other = root.path().join("other");
    std::fs::create_dir(&build).unwrap();
    std::fs::create_dir(&other).unwrap();
    let (binary, _, _) = support::compile(Language::C, &build, true);
    let (profdata, cov) = support::coverage_tools(Language::C, &build);
    let capture = CoverageBuild::bind(&binary, Language::C, &profdata, &cov, &build).unwrap();
    let input = root.path().join("assets");
    support::assets(&input);
    let output = root.path().join("output");
    std::fs::create_dir(&output).unwrap();
    let raw = root.path().join("code.profraw");
    let launch = hunin_core::execution::PreparedLaunch {
        program: binary.clone(),
        args: vec![
            input.display().to_string(),
            output.display().to_string(),
            "single".into(),
        ],
        directory: root.path().into(),
        environment: BTreeMap::from([("LLVM_PROFILE_FILE".into(), raw.display().to_string())]),
        stdout_limit: 65536,
    };
    let mut process = spawn_prepared(&launch, OwnedScope::ProcessGroup).unwrap();
    process.await_ready(Duration::from_secs(10)).unwrap();
    assert!(
        capture
            .export_owned_capture(&raw, "before", root.path())
            .is_ok()
    );
    let oversized = root.path().join("oversized.profraw");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len(256 * 1024 * 1024 + 1)
        .unwrap();
    let started = Instant::now();
    assert_eq!(
        capture
            .export_owned_capture(&oversized, "after", root.path())
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    let (other_binary, _, _) = support::compile(Language::Cpp, &other, true);
    std::fs::rename(&other_binary, &binary).unwrap();
    assert_eq!(
        capture
            .export_owned_capture(&raw, "after", root.path())
            .unwrap_err()
            .kind,
        ErrorKind::SymbolMismatch
    );
    let outcome = process
        .cancel(Duration::from_secs(1), Duration::from_secs(2))
        .unwrap();
    assert!(!outcome.cleanup_pending);
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "requires an explicitly provisioned PERFMON environment"]
fn real_perf_sample_limit_stops_collection_and_keeps_other_evidence() {
    let root = tempfile::tempdir().unwrap();
    let build = root.path().join("build");
    std::fs::create_dir(&build).unwrap();
    let (binary, _, _) = support::compile(Language::C, &build, false);
    let input = root.path().join("assets");
    let expected = support::assets(&input);
    let output = root.path().join("output");
    std::fs::create_dir(&output).unwrap();
    let launch = hunin_core::execution::PreparedLaunch {
        program: binary,
        args: vec![
            input.display().to_string(),
            output.display().to_string(),
            "single".into(),
        ],
        directory: root.path().into(),
        environment: BTreeMap::from([("PREVIEW_REPETITIONS".into(), "100000".into())]),
        stdout_limit: 65536,
    };
    let mut process = spawn_prepared(&launch, OwnedScope::ProcessGroup).unwrap();
    process.await_ready(Duration::from_secs(10)).unwrap();
    let instance = inspect_process(process.id()).unwrap();
    let project = Project::create(root.path(), "p").unwrap();
    let investigation = project
        .create_investigation("i", "bounded", json!({}))
        .unwrap();
    let run = project.create_run(&investigation, "r", json!({})).unwrap();
    let target = project
        .register_target(
            &run,
            "target",
            serde_json::to_value(&instance).unwrap(),
            true,
        )
        .unwrap();
    let first = read_resources(&instance).unwrap();
    let resources = project
        .register_source(
            &run,
            hunin_platforms::recording::resource_definition("resources", target.clone(), &first),
        )
        .unwrap();
    resources
        .append(&[first.to_record("known", "target").unwrap()])
        .unwrap();
    resources.ready(json!({})).unwrap();
    let mut collector =
        hunin_platforms::native::perf::PerfCollector::start(&instance, Language::C, 999, 1, 128)
            .unwrap();
    let code = project
        .register_source(&run, collector.definition("code", target))
        .unwrap();
    code.ready(json!({})).unwrap();
    process.release().unwrap();
    let start = Instant::now();
    let error = loop {
        match collector.drain() {
            Ok(_) => {}
            Err(error) => break error,
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "source sample limit must fail finitely"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(error.kind, ErrorKind::LimitExceeded);
    drop(collector);
    assert!(
        inspect_process(instance.pid)
            .unwrap()
            .same_instance(&instance)
    );
    code.report_gap(
        "sample-cap",
        json!({"reason":error.message,"record_limit":1}),
    )
    .unwrap();
    code.close(json!({"complete":false})).unwrap();
    while !process.work_complete().unwrap() {
        assert!(start.elapsed() < Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(10));
    }
    support::check_outputs(&output, &expected);
    let last = read_resources(&instance).unwrap();
    resources
        .append(&[last.to_record("final", "target").unwrap()])
        .unwrap();
    resources
        .close(json!({"complete":true,"code_failure_does_not_erase_resources":true}))
        .unwrap();
    process.finish().unwrap();
    assert!(process.wait(Duration::from_secs(5)).unwrap().success);
    assert!(project.record(&resources.record_ref("known"), None).is_ok());
    assert!(code.status().unwrap().closed);
}

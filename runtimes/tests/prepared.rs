use hunin_core::{
    execution::PreparedProcess,
    project::Project,
    query::{Query, QueryLimits},
};
use hunin_runtimes::{Limits, RuntimeKind, prepare, python, ruby, v8};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

fn run(runtime_var: &str, fixture: &str) -> (Vec<u8>, Value) {
    let executable = PathBuf::from(
        std::env::var(runtime_var).expect("set the selected absolute runtime executable"),
    );
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let temp = tempfile::tempdir().unwrap();
    let kind = match runtime_var {
        "HUNIN_NODE" => RuntimeKind::Node,
        "HUNIN_DENO" => RuntimeKind::Deno,
        "HUNIN_BUN" => RuntimeKind::Bun,
        "HUNIN_PYTHON" => RuntimeKind::CPython,
        "HUNIN_PYPY" => RuntimeKind::PyPy,
        "HUNIN_RUBY" => RuntimeKind::CRuby,
        "HUNIN_JAVA" => RuntimeKind::HotSpot,
        "HUNIN_GO" => RuntimeKind::Go,
        _ => panic!("unsupported test runtime"),
    };
    let mut prepared = prepare(
        kind,
        &executable,
        &root.join("tests/fixtures").join(fixture),
        temp.path(),
    )
    .unwrap();
    prepared
        .launch
        .environment
        .insert("HUNIN_FINALIZE_GATE".into(), "1".into());
    assert!(!prepared.schemas().is_empty());
    let profile = prepared.profile_path.clone();
    let report = prepared.report_path.clone();
    let mut process = PreparedProcess::spawn(&prepared.launch).unwrap();
    let data = tempfile::tempdir().unwrap();
    let project = Project::create(data.path(), "runtime").unwrap();
    let investigation = project
        .create_investigation("i", "runtime source retention", json!({}))
        .unwrap();
    let run = project.create_run(&investigation, "r", json!({})).unwrap();
    let target = project
        .register_target(&run, "process", json!({"pid":process.id()}), true)
        .unwrap();
    let source = project
        .register_source(&run, prepared.definition("code", &target))
        .unwrap();
    process.await_ready(Duration::from_secs(10)).unwrap();
    source
        .ready(json!({"gate":"profiler ready before application release"}))
        .unwrap();
    assert!(
        !profile.exists() || fs::metadata(&profile).unwrap().len() == 0,
        "work cannot have completed before release"
    );
    process.release().unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    while !process.work_complete().unwrap() {
        assert!(Instant::now() < until, "final gate deadline");
        std::thread::sleep(Duration::from_millis(2));
    }
    let extraction = prepared.extract("test-process", Limits::default()).unwrap();
    for batch in extraction.records.chunks(256) {
        source.append(batch).unwrap();
    }
    source
        .close(json!({"records":extraction.records.len(),"warnings":extraction.warnings}))
        .unwrap();
    process.finish().unwrap();
    let outcome = process.wait(Duration::from_secs(30)).unwrap();
    assert!(outcome.success, "{outcome:?}");
    let report: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(report["outcome"], "completed");
    let pixels = report["output"]["pixels"].as_array().unwrap();
    for (i, pixel) in pixels.iter().enumerate() {
        assert_eq!(
            pixel.as_u64().unwrap(),
            ((((i & 255) * 3 + 7) ^ (i & 255)) & 255) as u64
        );
    }
    let bytes = fs::read(profile).unwrap();
    fs::remove_dir_all(temp.path()).unwrap();
    drop(source);
    drop(project);
    let reopened = Project::open(data.path(), "runtime").unwrap();
    let mut query = Query::new(run);
    query.source = Some("code".into());
    let mut restored = vec![];
    let mut cursor = None;
    loop {
        let page = reopened
            .query(&query, QueryLimits::default(), cursor.as_ref())
            .unwrap();
        for entry in page.records {
            restored.push(if let Some(record) = entry.record {
                record
            } else {
                let chunk = reopened.read_value(&entry.value, 0, 1024 * 1024).unwrap();
                assert!(chunk.next_offset.is_none());
                serde_json::from_slice(&chunk.bytes).unwrap()
            });
        }
        cursor = page.continuation;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        serde_json::to_value(&restored).unwrap(),
        serde_json::to_value(&extraction.records).unwrap()
    );
    if kind == RuntimeKind::Go {
        let costs = hunin_runtimes::pprof::costs(&restored, "cpu", "nanoseconds").unwrap();
        assert!(
            costs
                .functions
                .iter()
                .any(|f| f.name == "main.resize" && f.inclusive_value > 0)
        );
    }
    assert!(
        reopened
            .sources(&query.run, QueryLimits::default(), None)
            .unwrap()
            .items[0]
            .closed
    );
    (bytes, report)
}

#[test]
#[ignore = "requires HUNIN_NODE absolute selected runtime"]
fn real_node_profile_observes_prepared_pixel_work_after_the_gate() {
    let (bytes, _) = run("HUNIN_NODE", "preview.cjs");
    let extracted = v8::extract(&bytes, "node-process", Limits::default()).unwrap();
    assert!(extracted.records.iter().any(|r| {
        r.schema == "v8.sample"
            && r.function.as_deref() == Some("resize")
            && r.location
                .as_ref()
                .is_some_and(|p| p.contains("preview.cjs"))
    }));
}

#[test]
#[ignore = "requires HUNIN_DENO absolute selected runtime"]
fn real_deno_inspector_profile_retains_its_own_v8_source_identity() {
    let (bytes, report) = run("HUNIN_DENO", "preview.cjs");
    assert!(report["runtime"]["deno"].is_string());
    let extracted = v8::extract(&bytes, "deno-process", Limits::default()).unwrap();
    assert!(
        extracted
            .records
            .iter()
            .any(|r| r.schema == "v8.sample" && r.function.as_deref() == Some("resize"))
    );
}

#[test]
#[ignore = "requires HUNIN_BUN absolute selected runtime"]
fn real_bun_inspector_profile_is_stoppable_before_owned_exit() {
    let (bytes, report) = run("HUNIN_BUN", "preview.cjs");
    assert!(report["runtime"]["bun"].is_string());
    let extracted = v8::extract(&bytes, "bun-process", Limits::default()).unwrap();
    assert!(
        extracted
            .records
            .iter()
            .any(|r| r.schema == "v8.sample" && r.function.as_deref() == Some("resize"))
    );
}

#[test]
#[ignore = "requires HUNIN_RUBY absolute runtime"]
fn real_ruby_vm_events_record_calls_independently_of_operation_markers() {
    let (bytes, _) = run("HUNIN_RUBY", "preview.rb");
    let extracted = ruby::extract(&bytes, "ruby-process", Limits::default()).unwrap();
    assert_eq!(
        extracted
            .records
            .iter()
            .filter(|r| r.schema == "ruby.event"
                && r.payload["event"] == "call"
                && r.function.as_deref() == Some("resize"))
            .count(),
        7
    );
    let header = extracted.records.iter().find(|r| r.id == "trace").unwrap();
    assert_eq!(header.payload["lost_events"], 0);
}

#[test]
#[ignore = "requires HUNIN_PYTHON absolute selected runtime"]
fn real_python_profile_keeps_the_complete_caller_graph() {
    let (bytes, _) = run("HUNIN_PYTHON", "preview.py");
    let extracted = python::extract(&bytes, "python-process", Limits::default()).unwrap();
    let resize = extracted
        .records
        .iter()
        .find(|r| r.schema == "python.cprofile-function" && r.function.as_deref() == Some("resize"))
        .unwrap();
    assert_eq!(resize.payload["calls"], 7);
    assert!(
        extracted
            .records
            .iter()
            .any(|r| r.schema == "python.cprofile-edge"
                && r.payload["callee"] == resize.payload["id"]
                && r.payload["calls"] == 7)
    );
}

#[test]
#[ignore = "requires HUNIN_PYPY absolute selected runtime"]
fn real_pypy_profile_retains_jit_runtime_identity_and_call_edges() {
    let (bytes, _) = run("HUNIN_PYPY", "preview.py");
    let extraction = python::extract(&bytes, "pypy-process", Limits::default()).unwrap();
    let profile = extraction
        .records
        .iter()
        .find(|r| r.id == "profile")
        .unwrap();
    assert_eq!(profile.payload["runtime"], "pypy");
    assert!(
        extraction
            .records
            .iter()
            .any(|r| r.function.as_deref() == Some("resize") && r.payload["calls"] == 7)
    );
}

#[test]
#[ignore = "requires HUNIN_JAVA absolute selected full-JDK runtime"]
fn real_jfr_samples_keep_java_method_source_and_stack_evidence() {
    let (bytes, _) = run("HUNIN_JAVA", "Preview.java");
    let extraction = hunin_runtimes::jfr::extract(&bytes, "jvm", Limits::default()).unwrap();
    assert!(extraction.records.iter().any(|r| {
        r.schema == "jfr.frame"
            && r.function.as_deref() == Some("Preview.resize")
            && r.location
                .as_ref()
                .is_some_and(|s| s.contains("Preview.java:"))
    }));
}

#[test]
#[ignore = "requires HUNIN_GO absolute selected toolchain"]
fn real_go_profile_keeps_aggregate_cpu_units_inline_stacks_and_source() {
    let (bytes, report) = run("HUNIN_GO", "preview.go");
    assert!(report["runtime"].as_str().unwrap().starts_with("go1."));
    let extraction = hunin_runtimes::pprof::extract(&bytes, "go", Limits::default()).unwrap();
    let header = extraction
        .records
        .iter()
        .find(|r| r.schema == "pprof.profile")
        .unwrap();
    assert_eq!(header.payload["sample_types"][1]["unit"], "nanoseconds");
    assert!(
        extraction
            .records
            .iter()
            .any(|r| r.schema == "pprof.sample" && r.payload["values"][1].as_i64().unwrap() > 0)
    );
    assert!(extraction.records.iter().any(|r| {
        r.schema == "pprof.location"
            && r.function.as_deref() == Some("main.resize")
            && r.location
                .as_ref()
                .is_some_and(|s| s.contains("preview.go:"))
    }));
}

#[test]
#[ignore = "requires selected HUNIN_RUBY"]
fn ruby_recursion_exception_thread_and_fiber_events_preserve_the_program_result() {
    let exe = PathBuf::from(std::env::var("HUNIN_RUBY").unwrap());
    let area = tempfile::tempdir().unwrap();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/control.rb");
    let prepared = prepare(RuntimeKind::CRuby, &exe, &path, area.path()).unwrap();
    let mut process = PreparedProcess::spawn(&prepared.launch).unwrap();
    process.await_ready(Duration::from_secs(10)).unwrap();
    process.release().unwrap();
    let outcome = process.wait(Duration::from_secs(10)).unwrap();
    assert!(outcome.success, "{outcome:?}");
    let extraction = prepared.extract("ruby", Limits::default()).unwrap();
    let report: Value = serde_json::from_slice(&fs::read(prepared.report_path).unwrap()).unwrap();
    assert_eq!(
        report["output"],
        json!({"recursion":4,"rescued":true,"thread":28,"fiber_first":13,"fiber_second":16})
    );
    assert_eq!(
        extraction
            .records
            .iter()
            .filter(|r| r.schema == "ruby.event"
                && r.function.as_deref() == Some("recurse")
                && r.payload["event"] == "call")
            .count(),
        5
    );
    assert!(
        extraction
            .records
            .iter()
            .any(|r| r.payload["event"] == "raise" && r.function.as_deref() == Some("fail_once"))
    );
    assert!(
        extraction
            .records
            .iter()
            .any(|r| r.payload["event"] == "thread_begin")
    );
    assert!(
        extraction
            .records
            .iter()
            .any(|r| r.payload["event"] == "fiber_switch")
    );
    let work = extraction
        .records
        .iter()
        .filter(|r| r.function.as_deref() == Some("work") && r.payload["event"] == "call")
        .collect::<Vec<_>>();
    assert_eq!(work.len(), 3);
    assert!(
        work.iter()
            .map(|r| r.payload["thread"].as_u64().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            > 1
    );
    assert!(
        work.iter()
            .map(|r| r.payload["fiber"].as_u64().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            > 1
    );
}

#[test]
#[ignore = "requires selected HUNIN_PYTHON and HUNIN_PYPY"]
fn python_native_code_identity_disambiguates_same_line_lambdas() {
    for (kind, key) in [
        (RuntimeKind::CPython, "HUNIN_PYTHON"),
        (RuntimeKind::PyPy, "HUNIN_PYPY"),
    ] {
        let exe = PathBuf::from(std::env::var(key).unwrap());
        let area = tempfile::tempdir().unwrap();
        let workload =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/identities.py");
        let prepared = prepare(kind, &exe, &workload, area.path()).unwrap();
        let mut process = PreparedProcess::spawn(&prepared.launch).unwrap();
        process.await_ready(Duration::from_secs(10)).unwrap();
        process.release().unwrap();
        assert!(process.wait(Duration::from_secs(10)).unwrap().success);
        let extraction = prepared.extract("python", Limits::default()).unwrap();
        assert!(extraction.capture_complete);
        let functions = extraction
            .records
            .iter()
            .filter(|r| {
                r.schema == "python.cprofile-function" && r.function.as_deref() == Some("<lambda>")
            })
            .collect::<Vec<_>>();
        assert_eq!(functions.len(), 2);
        assert_eq!(functions[0].payload["line"], functions[1].payload["line"]);
        assert_ne!(functions[0].payload["id"], functions[1].payload["id"]);
        for function in functions {
            assert!(
                extraction
                    .records
                    .iter()
                    .any(|r| r.schema == "python.cprofile-edge"
                        && r.payload["callee"] == function.payload["id"]
                        && r.payload["callee_resolution"] == "object_identity"
                        && r.payload["calls"] == 1)
            );
        }
    }
}

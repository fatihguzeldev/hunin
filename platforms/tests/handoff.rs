use hunin_core::{
    control::{Action, Controller, RequestId},
    execution::PreparedLaunch,
    project::{Project, Reference, TargetRef},
    query::{Context, Direction, FieldFilter, Origin, Query, QueryLimits, Relationship},
    records::{
        Clock, FieldSchema, FieldType, Record, RecordSchema, RecordTime, SourceDefinition,
        SourceHandle,
    },
};
#[cfg(target_os = "macos")]
use hunin_platforms::native::coverage::CoverageBuild;
use hunin_platforms::{
    ProcessInstance, inspect_process,
    native::{
        Language,
        control::{OwnedScope, spawn_prepared},
        coverage::sha256,
    },
    read_resources, tools,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
fn tool(name: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("missing {name}"))
}
fn output(command: &mut Command, area: &Path) -> String {
    tools::require_success(tools::run(command, area, Duration::from_secs(30), 1024 * 1024).unwrap())
        .unwrap()
        .stdout
}
#[cfg(target_os = "macos")]
fn matching_tools(area: &Path) -> (PathBuf, PathBuf) {
    let sysroot = output(
        Command::new(tool("rustc")).args(["--print", "sysroot"]),
        area,
    );
    let version = output(
        Command::new(tool("rustc")).args(["--version", "--verbose"]),
        area,
    );
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .unwrap();
    let directory = Path::new(sysroot.trim())
        .join("lib/rustlib")
        .join(host)
        .join("bin");
    (directory.join("llvm-profdata"), directory.join("llvm-cov"))
}
fn append(source: &SourceHandle, records: &[Record]) {
    for chunk in records.chunks(32) {
        source.append(chunk).unwrap();
    }
}
struct Code {
    source: SourceHandle,
    #[cfg(target_os = "macos")]
    build: CoverageBuild,
    #[cfg(target_os = "macos")]
    raw: PathBuf,
    #[cfg(target_os = "macos")]
    area: PathBuf,
    #[cfg(target_os = "linux")]
    profiler: hunin_platforms::native::perf::PerfCollector,
    #[cfg(windows)]
    profiler: hunin_platforms::native::windows::EtwRecording,
    binary: PathBuf,
}
impl Code {
    #[allow(clippy::too_many_arguments)]
    fn start(
        project: &Project,
        run: &Reference,
        target: TargetRef,
        instance: &ProcessInstance,
        binary: &Path,
        area: &Path,
        raw: &Path,
    ) -> Self {
        let id = format!("{}-code", target.id);
        #[cfg(target_os = "macos")]
        {
            let _ = instance;
            let (profdata, cov) = matching_tools(area);
            let build = CoverageBuild::bind(binary, Language::Rust, &profdata, &cov, area).unwrap();
            let before = build.export_owned_capture(raw, "before", area).unwrap();
            let records = before.records(&target.id);
            assert!(records.iter().any(|record| {
                record
                    .function
                    .as_ref()
                    .is_some_and(|name| name.contains("thread_work"))
                    && record.schema.ends_with(".function")
                    && record.payload["count"] == 0
            }));
            assert!(records.iter().any(|record| {
                record
                    .function
                    .as_ref()
                    .is_some_and(|name| name.contains("process_work"))
                    && record.schema.ends_with(".function")
                    && record.payload["count"] == 0
            }));
            let source = project
                .register_source(run, before.definition(&id, target.clone()))
                .unwrap();
            append(&source, &records);
            source.ready(json!({"basis":"same binary hash and zero work counters before GO","aggregate_scope":"one process including its threads; no cross-process time alignment"})).unwrap();
            Self {
                source,
                build,
                raw: raw.into(),
                area: area.into(),
                binary: binary.into(),
            }
        }
        #[cfg(target_os = "linux")]
        {
            let _ = (area, raw);
            let profiler = hunin_platforms::native::perf::PerfCollector::start(
                instance,
                Language::Rust,
                99,
                100000,
                128,
            )
            .unwrap();
            let source = project
                .register_source(run, profiler.definition(&id, target))
                .unwrap();
            source
                .ready(json!({"basis":"all selected thread/cpu events enabled before GO"}))
                .unwrap();
            Self {
                source,
                profiler,
                binary: binary.into(),
            }
        }
        #[cfg(windows)]
        {
            let _ = raw;
            let profiler =
                hunin_platforms::native::windows::EtwRecording::start(instance, area).unwrap();
            let source = project
                .register_source(run, profiler.definition(&id, target).unwrap())
                .unwrap();
            source
                .ready(json!({"basis":"owned ETW session and stack tracing enabled before GO"}))
                .unwrap();
            Self {
                source,
                profiler,
                binary: binary.into(),
            }
        }
    }
    fn poll(&mut self) {
        #[cfg(target_os = "linux")]
        self.profiler.drain().unwrap();
    }
    fn finish(self, subject: &str, area: &Path) -> (SourceHandle, Vec<Record>) {
        #[cfg(target_os = "macos")]
        let records = {
            let _ = (&self.binary, area);
            self.build
                .export_owned_capture(&self.raw, "after", &self.area)
                .unwrap()
                .records(subject)
        };
        #[cfg(target_os = "linux")]
        let records = {
            let mut profile = self.profiler.stop().unwrap();

            hunin_platforms::native::symbols::resolve_elf_profile(&mut profile, &self.binary, area)
                .unwrap();
            profile.records(subject).unwrap()
        };
        #[cfg(windows)]
        let records = {
            let _ = area;
            let mut profile = self.profiler.stop(Language::Rust).unwrap();
            hunin_platforms::native::windows::resolve_pdb_profile(
                &mut profile,
                &self.binary,
                &self.binary.with_extension("pdb"),
            )
            .unwrap();
            profile.records(subject).unwrap()
        };
        append(&self.source, &records);
        self.source.close(json!({"complete":true})).unwrap();
        (self.source, records)
    }
}
fn instrumentation(parent: u32, child: u32) -> SourceDefinition {
    let fields = BTreeMap::from([
        (
            "pid".into(),
            FieldSchema {
                kind: FieldType::Unsigned,
                required: true,
                unit: None,
                meaning: "actual fixture process id, separately bound to its stable core target"
                    .into(),
            },
        ),
        (
            "value".into(),
            FieldSchema {
                kind: FieldType::Unsigned,
                required: true,
                unit: Some("checksum".into()),
                meaning: "deterministic result from the executed worker".into(),
            },
        ),
        (
            "parent".into(),
            FieldSchema {
                kind: FieldType::Nullable(Box::new(FieldType::String)),
                required: true,
                unit: None,
                meaning: "explicit received causal token's logical parent; null remains unlinked"
                    .into(),
            },
        ),
        (
            "token".into(),
            FieldSchema {
                kind: FieldType::Nullable(Box::new(FieldType::String)),
                required: true,
                unit: None,
                meaning: "token actually transferred over a channel or process stdin".into(),
            },
        ),
    ]);
    SourceDefinition{id:"instrumentation".into(),target:None,schemas:vec![RecordSchema{name:"native.handoff.event".into(),version:1,fields,description:"fixture-origin instrumentation with explicit tokens and process-local intervals".into()}],clocks:[parent,child].into_iter().map(|pid|Clock{id:format!("fixture-instant-{pid}"),ticks_per_second:1_000_000_000,description:"std Instant from this process's fixture startup; separate process clocks have no measured alignment".into()}).collect(),interpretation:"native-handoff-fixture/1".into(),metadata:json!({"origin":"instrumented","clock_uncertainty_ns":1000000,"relationship_basis":"received token, never overlapping time intervals","input":"fixed wrapping integer recurrence, seeds1/2/3, fifty million iterations"})}
}
fn ingest_events(
    source: &SourceHandle,
    path: &Path,
    parent: u32,
    child: u32,
    seen: &mut BTreeMap<String, Record>,
) {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.len() < 65536);
    for line in text.lines() {
        let value: Value = serde_json::from_str(line).unwrap();
        let id = value["id"].as_str().unwrap().to_string();
        if seen.contains_key(&id) {
            continue;
        }
        let pid = value["pid"].as_u64().unwrap() as u32;
        assert!([parent, child].contains(&pid));
        let record = Record {
            id: id.clone(),
            schema: "native.handoff.event".into(),
            schema_version: 1,
            subject: if pid == parent { "parent" } else { "child" }.into(),
            operation: Some(id.clone()),
            function: None,
            location: None,
            time: Some(RecordTime {
                clock: format!("fixture-instant-{pid}"),
                start: value["start_ns"].as_i64().unwrap(),
                end: Some(value["end_ns"].as_i64().unwrap()),
                uncertainty: 1_000_000,
            }),
            payload: value,
        };
        source.append(std::slice::from_ref(&record)).unwrap();
        seen.insert(id, record);
    }
}
fn expected(mut n: u64) -> u64 {
    for i in 0..50_000_000 {
        n = n
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407)
            ^ i;
    }
    n
}
#[test]
#[ignore = "selected native host compiler/profiler environment; Linux requires PERFMON, macOS matching rust llvm-tools"]
fn explicit_thread_and_process_handoffs_survive_reopen_without_attributing_unlinked_work() {
    let root = tempfile::tempdir().unwrap();
    let project = Project::create(root.path(), "handoff").unwrap();
    let investigation = project
        .create_investigation("i", "native handoffs", json!({}))
        .unwrap();
    let run=project.create_run(&investigation,"run",json!({"source":"instrumented native handoff plus independent code and resource collectors","platform":std::env::consts::OS,"architecture":std::env::consts::ARCH})).unwrap();
    let controller = Controller::new(project.clone());
    let key = RequestId {
        project: project.id().into(),
        caller: "agent".into(),
        id: "launch".into(),
    };
    controller.accept(&key,&run,Action::Launch,json!({"fixture":"native thread/process handoff","owned_scope":"process group/job including launched child"})).unwrap();
    let launch_request = controller.dispatch(&key).unwrap();
    let area = tempfile::Builder::new()
        .prefix("handoff-")
        .tempdir_in(project.directory().join("tmp"))
        .unwrap();
    let binary = area.path().join(if cfg!(windows) {
        "handoff.exe"
    } else {
        "handoff"
    });
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/handoff.rs");
    let mut compiler = Command::new(tool("rustc"));
    compiler.args([
        "--edition=2024",
        "-g",
        "-C",
        "opt-level=1",
        "-C",
        "force-frame-pointers=yes",
    ]);
    if cfg!(target_os = "macos") {
        compiler.args(["-C", "instrument-coverage", "--cfg", "hunin_coverage"]);
    }
    if cfg!(target_os = "linux") {
        compiler.args(["-C", "link-arg=-Wl,--build-id=sha1"]);
    }
    output(compiler.arg(&source).arg("-o").arg(&binary), area.path());
    let binary_hash = sha256(&binary).unwrap();
    project.add_context(&run,Context{id:"build".into(),subject:"fixture".into(),origin:Origin::Contributed,value:json!({"binary_sha256":binary_hash,"source_sha256":sha256(&source).unwrap(),"compiler":output(Command::new(tool("rustc")).args(["--version","--verbose"]),area.path())}),corrects:None}).unwrap();
    let launch = PreparedLaunch {
        program: binary.clone(),
        args: vec![area.path().display().to_string()],
        directory: area.path().into(),
        environment: BTreeMap::from([(
            "LLVM_PROFILE_FILE".into(),
            area.path().join("parent.profraw").display().to_string(),
        )]),
        stdout_limit: 65536,
    };
    let mut process = spawn_prepared(&launch, OwnedScope::ProcessGroup).unwrap();
    process.await_ready(Duration::from_secs(10)).unwrap();
    let parent = inspect_process(process.id()).unwrap();
    let child_pid: u32 = std::fs::read_to_string(area.path().join("child.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let child = inspect_process(child_pid).unwrap();
    #[cfg(unix)]
    assert_eq!(
        unsafe { libc::getpgid(child_pid as i32) },
        parent.pid as i32
    );
    let mut sources = Vec::new();
    let mut first = Vec::new();
    let mut targets = Vec::new();
    for (name, instance) in [("parent", &parent), ("child", &child)] {
        let target = project
            .register_target(&run, name, serde_json::to_value(instance).unwrap(), true)
            .unwrap();
        let snapshot = read_resources(instance).unwrap();
        let source = project
            .register_source(
                &run,
                hunin_platforms::recording::resource_definition(
                    &format!("{name}-resources"),
                    target.clone(),
                    &snapshot,
                ),
            )
            .unwrap();
        source
            .append(&[snapshot.to_record("0", name).unwrap()])
            .unwrap();
        source
            .ready(json!({"before_go":true,"instance":instance}))
            .unwrap();
        sources.push(source);
        first.push(snapshot);
        targets.push(target);
    }
    let mut parent_code = Code::start(
        &project,
        &run,
        targets[0].clone(),
        &parent,
        &binary,
        area.path(),
        &area.path().join("parent.profraw"),
    );
    let mut child_code = Code::start(
        &project,
        &run,
        targets[1].clone(),
        &child,
        &binary,
        area.path(),
        &area.path().join("child.profraw"),
    );
    let events = project
        .register_source(&run, instrumentation(parent.pid, child.pid))
        .unwrap();
    events
        .ready(json!({"basis":"instrumentation journal initialized before GO"}))
        .unwrap();
    process.release().unwrap();
    project.add_context(&run,Context{id:"live-context".into(),subject:"batch".into(),origin:Origin::Contributed,value:json!({"description":"agent-selected batch; unrelated sibling deliberately excluded"}),corrects:None}).unwrap();
    let mut seen = BTreeMap::new();
    let start = Instant::now();
    let mut sample = 1;
    let mut durable_live = false;
    while !process.work_complete().unwrap() {
        assert!(start.elapsed() < Duration::from_secs(20));
        ingest_events(
            &events,
            &area.path().join("events.jsonl"),
            parent.pid,
            child.pid,
            &mut seen,
        );
        if seen.contains_key("batch") {
            assert_eq!(
                project.record(&events.record_ref("batch"), None).unwrap(),
                seen["batch"]
            );
            durable_live = true;
        }
        for (index, instance) in [&parent, &child].into_iter().enumerate() {
            let reading = read_resources(instance).unwrap();
            sources[index]
                .append(&[reading
                    .to_record(
                        &sample.to_string(),
                        if index == 0 { "parent" } else { "child" },
                    )
                    .unwrap()])
                .unwrap();
        }
        sample += 1;
        parent_code.poll();
        child_code.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    ingest_events(
        &events,
        &area.path().join("events.jsonl"),
        parent.pid,
        child.pid,
        &mut seen,
    );
    assert!(
        durable_live,
        "at least batch observation must be durable before work completes"
    );
    assert_eq!(seen.len(), 4);
    let (parent_source, parent_records) = parent_code.finish("parent", area.path());
    let (child_source, child_records) = child_code.finish("child", area.path());
    let thread = parent_records
        .iter()
        .find(|record| {
            record
                .function
                .as_ref()
                .is_some_and(|function| function.contains("thread_work"))
                && record
                    .location
                    .as_ref()
                    .is_some_and(|location| location.contains("handoff.rs"))
                && (!cfg!(target_os = "macos") || record.id.starts_with("after-"))
                && (!record.schema.ends_with(".region")
                    || record.payload["count"].as_u64().unwrap() > 0)
        })
        .unwrap();
    let child_work = child_records
        .iter()
        .find(|record| {
            record
                .function
                .as_ref()
                .is_some_and(|function| function.contains("process_work"))
                && record
                    .location
                    .as_ref()
                    .is_some_and(|location| location.contains("handoff.rs"))
                && (!cfg!(target_os = "macos") || record.id.starts_with("after-"))
                && (!record.schema.ends_with(".region")
                    || record.payload["count"].as_u64().unwrap() > 0)
        })
        .unwrap_or_else(|| {
            panic!(
                "child process_work not resolved; actual functions: {:?}",
                child_records
                    .iter()
                    .filter_map(|record| record.function.as_ref())
                    .collect::<std::collections::BTreeSet<_>>()
            )
        });
    let noise = parent_records
        .iter()
        .find(|record| {
            record
                .function
                .as_ref()
                .is_some_and(|function| function.contains("unrelated_work"))
                && record
                    .location
                    .as_ref()
                    .is_some_and(|location| location.contains("handoff.rs"))
                && (!cfg!(target_os = "macos") || record.id.starts_with("after-"))
                && (!record.schema.ends_with(".region")
                    || record.payload["count"].as_u64().unwrap() > 0)
        })
        .unwrap();
    assert_eq!(seen["thread-work"].payload["value"], expected(1));
    assert_eq!(seen["unrelated"].payload["value"], expected(2));
    assert_eq!(seen["process-work"].payload["value"], expected(3));
    let thread_time = seen["thread-work"].time.as_ref().unwrap();
    let noise_time = seen["unrelated"].time.as_ref().unwrap();
    assert_eq!(thread_time.clock, noise_time.clock);
    assert!(
        thread_time.start.max(noise_time.start) + 2_000_000
            < thread_time.end.unwrap().min(noise_time.end.unwrap()) - 2_000_000,
        "unrelated work actually overlaps, beyond stated timestamp uncertainty"
    );
    for id in ["thread-work", "process-work"] {
        let value = &seen[id].payload;
        assert_eq!(value["parent"], "batch");
        assert!(value["token"].as_str().unwrap().starts_with("batch:"));
        project
            .add_relationship(
                &run,
                Relationship {
                    id: format!("handoff-{id}"),
                    from: events.record_ref("batch"),
                    to: events.record_ref(id),
                    kind: "explicit-work-handoff".into(),
                    origin: Origin::Instrumented,
                    corrects: None,
                    metadata: json!({"token":value["token"],"handoff":value["handoff"]}),
                },
            )
            .unwrap();
        assert_eq!(
            project
                .relationships(
                    &events.record_ref(id),
                    Direction::Reverse,
                    None,
                    QueryLimits::default(),
                    None
                )
                .unwrap()
                .relationships
                .len(),
            1
        );
    }
    for (operation, source, code) in [
        ("thread-work", &parent_source, thread),
        ("process-work", &child_source, child_work),
    ] {
        project.add_relationship(&run,Relationship{id:format!("code-{operation}"),from:events.record_ref(operation),to:source.record_ref(&code.id),kind:"corroborated-function-in-process-run".into(),origin:Origin::Contributed,corrects:None,metadata:json!({"scope":"known function in that process's measured run; aggregate evidence does not imply exclusive cpu ownership"})}).unwrap();
        assert_eq!(
            project
                .relationships(
                    &events.record_ref(operation),
                    Direction::Forward,
                    None,
                    QueryLimits::default(),
                    None
                )
                .unwrap()
                .relationships
                .len(),
            1
        );
        assert_eq!(
            project
                .relationships(
                    &source.record_ref(&code.id),
                    Direction::Reverse,
                    None,
                    QueryLimits::default(),
                    None
                )
                .unwrap()
                .relationships
                .len(),
            1
        );
    }
    assert_eq!(
        project
            .relationships(
                &events.record_ref("batch"),
                Direction::Forward,
                None,
                QueryLimits::default(),
                None
            )
            .unwrap()
            .relationships
            .len(),
        2
    );
    assert!(
        project
            .relationships(
                &events.record_ref("unrelated"),
                Direction::Reverse,
                None,
                QueryLimits::default(),
                None
            )
            .unwrap()
            .relationships
            .is_empty()
    );
    assert!(
        project
            .relationships(
                &parent_source.record_ref(&noise.id),
                Direction::Reverse,
                None,
                QueryLimits::default(),
                None
            )
            .unwrap()
            .relationships
            .is_empty()
    );
    for (index, instance) in [&parent, &child].into_iter().enumerate() {
        let last = read_resources(instance).unwrap();
        assert!(
            last.user_cpu.nanoseconds().unwrap() + last.system_cpu.nanoseconds().unwrap()
                > first[index].user_cpu.nanoseconds().unwrap()
                    + first[index].system_cpu.nanoseconds().unwrap()
        );
        sources[index]
            .append(&[last
                .to_record("final", if index == 0 { "parent" } else { "child" })
                .unwrap()])
            .unwrap();
        sources[index].close(json!({"complete":true,"scope":"one process all threads; no exclusive attribution to linked operation"})).unwrap();
    }
    events.close(json!({"complete":true})).unwrap();
    let retained = vec![
        (events.record_ref("batch"), seen["batch"].clone()),
        (
            events.record_ref("thread-work"),
            seen["thread-work"].clone(),
        ),
        (
            events.record_ref("process-work"),
            seen["process-work"].clone(),
        ),
        (events.record_ref("unrelated"), seen["unrelated"].clone()),
        (parent_source.record_ref(&thread.id), thread.clone()),
        (child_source.record_ref(&child_work.id), child_work.clone()),
    ];
    process.finish().unwrap();
    assert!(process.wait(Duration::from_secs(5)).unwrap().success);
    controller.complete(&key,launch_request.generation,true,json!({"application_outcome":"completed","resources_and_code":"both stable target instances","linked_operations":2,"unlinked_operations":1})).unwrap();
    drop(area);
    drop(parent_source);
    drop(child_source);
    drop(events);
    drop(sources);
    drop(controller);
    drop(project);
    let reopened = Project::open(root.path(), "handoff").unwrap();
    for (reference, record) in &retained {
        assert_eq!(reopened.record(reference, None).unwrap(), *record);
    }
    assert_eq!(
        reopened
            .relationships(
                &retained[0].0,
                Direction::Forward,
                None,
                QueryLimits::default(),
                None
            )
            .unwrap()
            .relationships
            .len(),
        2
    );
    let source = reopened.source(&run, "instrumentation").unwrap();
    assert_eq!(source.append(&[retained[1].1.clone()]).unwrap().replayed, 1);
    let mut query = Query::new(run);
    query.source = Some("instrumentation".into());
    query.fields.push(FieldFilter {
        name: "value".into(),
        value: seen["thread-work"].payload["value"].clone(),
    });
    assert_eq!(
        reopened
            .query(&query, QueryLimits::default(), None)
            .unwrap()
            .records
            .len(),
        1
    );
    assert_eq!(
        std::fs::read_dir(reopened.directory().join("tmp"))
            .unwrap()
            .count(),
        0
    );
}

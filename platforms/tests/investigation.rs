mod support;
use hunin_core::{
    compare::{CheckOutcome, ComparisonStatus, MeasurementMeaning, Series, Trial, compare},
    control::{Action, Controller, RequestId},
    execution::PreparedLaunch,
    project::Project,
    query::{Context, Direction, Origin, Query, QueryLimits, RecordRef, Relationship},
    records::{FieldSchema, FieldType, Record, RecordSchema, SourceDefinition, SourceHandle},
};
#[cfg(target_os = "macos")]
use hunin_platforms::native::coverage::CoverageBuild;
use hunin_platforms::{
    inspect_process,
    native::{
        Language,
        control::{OwnedScope, spawn_prepared},
        coverage::sha256,
    },
    read_resources,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};
#[cfg(target_os = "macos")]
use support::coverage_tools;
use support::{assets, check_outputs, compile};

fn append(source: &SourceHandle, records: &[Record]) {
    for chunk in records.chunks(32) {
        source.append(chunk).unwrap();
    }
}
fn operation_source() -> SourceDefinition {
    SourceDefinition {
        id: "contributions".into(),
        target: None,
        clocks: Vec::new(),
        interpretation: "preview-fixture-driver/1".into(),
        metadata: json!({"origin":"contributed","meaning":"caller-defined application scope; not automatically inferred ownership"}),
        schemas: vec![RecordSchema {
            name: "preview.operation".into(),
            version: 1,
            description: "agent-defined batch and asset meaning".into(),
            fields: BTreeMap::from([(
                "asset".into(),
                FieldSchema {
                    kind: FieldType::String,
                    required: true,
                    unit: None,
                    meaning: "caller-selected input asset".into(),
                },
            )]),
        }],
    }
}
fn context_record() -> Record {
    Record {
        id: "batch".into(),
        schema: "preview.operation".into(),
        schema_version: 1,
        subject: "preview-batch".into(),
        operation: Some("preview-batch".into()),
        time: None,
        function: None,
        location: None,
        payload: json!({"asset":"alpha.ppm","origin":"contributed","scope":"one whole batch containing this asset; aggregate code evidence does not isolate this asset's cpu"}),
    }
}
fn request(project: &Project, id: &str) -> RequestId {
    RequestId {
        project: project.id().into(),
        caller: "native-agent".into(),
        id: id.into(),
    }
}

fn investigation(language: Language) {
    let root = tempfile::tempdir().unwrap();
    let build = root.path().join("build");
    std::fs::create_dir(&build).unwrap();
    let (binary, compiler, build_arguments) = compile(language, &build, cfg!(target_os = "macos"));
    let binary_hash = sha256(&binary).unwrap();
    let input = root.path().join("assets");
    let expected = assets(&input);
    let input_hashes: Vec<_> = expected
        .iter()
        .map(|(name, _, _, _)| json!({"name":name,"sha256":sha256(&input.join(name)).unwrap()}))
        .collect();
    let project = Project::create(root.path(), "native").unwrap();
    let investigation = project
        .create_investigation(
            "i",
            "directory preview batch",
            json!({"input":input_hashes,"meaning":"fixture domain, supplied by the agent"}),
        )
        .unwrap();
    let controller = Controller::new(project.clone());
    let mut trials = Vec::new();
    let mut retained: Vec<(RecordRef, Record)> = Vec::new();
    #[cfg(target_os = "macos")]
    let coverage = {
        let (profdata, cov) = coverage_tools(language, &build);
        CoverageBuild::bind(&binary, language, &profdata, &cov, &build).unwrap()
    };
    for variant in ["single", "redundant"] {
        for repetition in 0..2 {
            let id = format!("{variant}-{repetition}");
            let run=project.create_run(&investigation,&id,json!({"inputs":input_hashes,"variant":variant,"repetitions":10000,"compiler":compiler,"build_arguments":build_arguments,"binary_sha256":binary_hash,"declared_build_language":format!("{language:?}"),"source_sha256":sha256(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(if language==Language::Rust{"preview.rs"}else{"preview.c"})).unwrap(),"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH})).unwrap();
            let key = request(&project, &format!("launch-{id}"));
            let intent = json!({"program":binary,"binary_sha256":binary_hash,"args":[input,variant],"owned_scope":"process group or job"});
            assert!(
                controller
                    .accept(&key, &run, Action::Launch, intent.clone())
                    .unwrap()
                    .newly_accepted
            );
            let accepted = controller.dispatch(&key).unwrap();
            let area = tempfile::Builder::new()
                .prefix("native-")
                .tempdir_in(project.directory().join("tmp"))
                .unwrap();
            let output = area.path().join("output");
            std::fs::create_dir(&output).unwrap();
            let raw = area.path().join("coverage.profraw");
            let launch = PreparedLaunch {
                program: binary.clone(),
                args: vec![
                    input.display().to_string(),
                    output.display().to_string(),
                    variant.into(),
                ],
                directory: area.path().into(),
                environment: BTreeMap::from([
                    ("PREVIEW_REPETITIONS".into(), "10000".into()),
                    ("LLVM_PROFILE_FILE".into(), raw.display().to_string()),
                ]),
                stdout_limit: 65536,
            };
            let mut process = spawn_prepared(&launch, OwnedScope::ProcessGroup).unwrap();
            process.await_ready(Duration::from_secs(10)).unwrap();
            let instance = inspect_process(process.id()).unwrap();
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
                    hunin_platforms::recording::resource_definition(
                        "resources",
                        target.clone(),
                        &first,
                    ),
                )
                .unwrap();
            resources
                .append(&[first.to_record("0", "target").unwrap()])
                .unwrap();
            resources
                .ready(json!({"acquired":first.acquired,"before_measured_release":true}))
                .unwrap();
            #[cfg(target_os = "macos")]
            let code = {
                let before = coverage
                    .export_owned_capture(&raw, "before", area.path())
                    .unwrap();
                let before_records = before.records("target");
                assert!(
                    before_records
                        .iter()
                        .any(|record| record.schema.ends_with(".function")
                            && record
                                .function
                                .as_ref()
                                .is_some_and(|function| function.contains("resize_preview"))
                            && record.payload["count"] == 0),
                    "known entry counter zero before GO"
                );
                let source = project
                    .register_source(&run, before.definition("code", target.clone()))
                    .unwrap();
                append(&source, &before_records);
                source.ready(json!({"basis":"compiler-instrumented target flushed zero known function counter before READY","binary_sha256":coverage.sha256,"aggregate_only":true})).unwrap();
                source
            };
            #[cfg(target_os = "linux")]
            let mut profiler = hunin_platforms::native::perf::PerfCollector::start(
                &instance, language, 99, 100000, 128,
            )
            .unwrap();
            #[cfg(target_os = "linux")]
            let code = {
                let source = project
                    .register_source(&run, profiler.definition("code", target.clone()))
                    .unwrap();
                source.ready(json!({"basis":"every selected cpu/thread event enabled successfully before work release","method":"perf_event_open cpu-clock frame-pointer callchains"})).unwrap();
                source
            };
            #[cfg(windows)]
            let profiler =
                hunin_platforms::native::windows::EtwRecording::start(&instance, area.path())
                    .unwrap();
            #[cfg(windows)]
            let code = project
                .register_source(&run, profiler.definition("code", target.clone()).unwrap())
                .unwrap();
            let contributions = project.register_source(&run, operation_source()).unwrap();
            contributions.append(&[context_record()]).unwrap();
            contributions
                .ready(json!({"origin":"caller driver"}))
                .unwrap();
            process.release().unwrap();
            project.add_context(&run,Context{id:"live-asset-label".into(),subject:"preview-batch".into(),origin:Origin::Contributed,value:json!({"asset":"alpha.ppm","selection":"agent supplied while execution is running"}),corrects:None}).unwrap();
            let started = Instant::now();
            let mut readings = 1;
            while !process.work_complete().unwrap_or_else(|error| {
                panic!(
                    "{error}; target outcome: {:?}",
                    process.wait(Duration::from_secs(1))
                )
            }) {
                assert!(
                    started.elapsed() < Duration::from_secs(20),
                    "native fixture work deadline"
                );
                std::thread::sleep(Duration::from_millis(20));
                let current = read_resources(&instance).unwrap();
                resources
                    .append(&[current.to_record(&readings.to_string(), "target").unwrap()])
                    .unwrap();
                readings += 1;
                #[cfg(target_os = "linux")]
                profiler.drain().unwrap();
            }
            let current = read_resources(&instance).unwrap();
            resources
                .append(&[current.to_record(&readings.to_string(), "target").unwrap()])
                .unwrap();
            let last = current;
            readings += 1;
            #[cfg(target_os = "macos")]
            let code_records = {
                let after = coverage
                    .export_owned_capture(&raw, "after", area.path())
                    .unwrap();
                let records = after.records("target");
                let count = 3 * 10000 * if variant == "single" { 1 } else { 2 };
                assert!(records.iter().any(|record| {
                    record.schema.ends_with(".function")
                        && record
                            .function
                            .as_ref()
                            .is_some_and(|function| function.contains("resize_preview"))
                        && record.payload["count"] == count
                }));
                records
            };
            #[cfg(target_os = "linux")]
            let code_records = {
                let mut profile = profiler.stop().unwrap();
                assert!(!profile.samples.is_empty());
                hunin_platforms::native::symbols::resolve_elf_profile(
                    &mut profile,
                    &binary,
                    area.path(),
                )
                .unwrap();
                assert!(
                    profile
                        .samples
                        .iter()
                        .all(|sample| sample.timestamp >= profile.start.unwrap())
                );
                profile.records("target").unwrap()
            };
            #[cfg(windows)]
            let code_records = {
                let mut profile = profiler.stop(language).unwrap();
                hunin_platforms::native::windows::resolve_pdb_profile(
                    &mut profile,
                    &binary,
                    &binary.with_extension("pdb"),
                )
                .unwrap();
                profile.records("target").unwrap()
            };
            append(&code, &code_records);
            let known = code_records
                .iter()
                .find(|record| {
                    record
                        .function
                        .as_ref()
                        .is_some_and(|name| name.contains("resize_preview"))
                        && record.location.as_ref().is_some_and(|path| {
                            path.contains(if language == Language::Rust {
                                "preview.rs"
                            } else {
                                "preview.c"
                            })
                        })
                        && !record.id.starts_with("before-")
                })
                .expect("automatic function and source record");
            let known_ref = code.record_ref(&known.id);
            project.add_relationship(&run,Relationship{id:"selected-batch-evidence".into(),from:contributions.record_ref("batch"),to:known_ref.clone(),kind:"observed-within-batch-scope".into(),origin:Origin::Contributed,corrects:None,metadata:json!({"scope":"whole measured batch","no_per_asset_cpu_attribution":true})}).unwrap();
            assert_eq!(
                project
                    .relationships(
                        &known_ref,
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
            assert_eq!(
                project
                    .relationships(
                        &contributions.record_ref("batch"),
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
            let checks = check_outputs(&output, &expected);
            project
                .add_context(
                    &run,
                    Context {
                        id: "output-check".into(),
                        subject: "preview-batch".into(),
                        origin: Origin::Contributed,
                        value: checks,
                        corrects: None,
                    },
                )
                .unwrap();
            let mut query = Query::new(run.clone());
            query.source = Some("code".into());
            query.function = known.function.clone();
            assert!(
                !project
                    .query(&query, QueryLimits::default(), None)
                    .unwrap()
                    .records
                    .is_empty()
            );
            let mut operation = Query::new(run.clone());
            operation.operation = Some("preview-batch".into());
            assert_eq!(
                project
                    .query(&operation, QueryLimits::default(), None)
                    .unwrap()
                    .records
                    .len(),
                1
            );
            let cpu = last.user_cpu.nanoseconds().unwrap() + last.system_cpu.nanoseconds().unwrap()
                - first.user_cpu.nanoseconds().unwrap()
                - first.system_cpu.nanoseconds().unwrap();
            assert!(cpu > 0);
            assert!(readings >= 2);
            let elapsed = process.acknowledged_elapsed().unwrap();
            code.close(json!({"complete":true,"evidence":"function/source and original source fields retained"})).unwrap();
            resources
                .close(json!({"complete":true,"last_acquisition":last.acquired}))
                .unwrap();
            contributions.close(json!({"complete":true})).unwrap();
            process.finish().unwrap();
            assert!(process.wait(Duration::from_secs(5)).unwrap().success);
            let result = json!({"application_outcome":"completed","cpu_ns":cpu,"elapsed_ns":elapsed.as_nanos().to_string(),"output_checks":"passed","collection_complete":true});
            assert_eq!(
                controller
                    .complete(&key, accepted.generation, true, result)
                    .unwrap()
                    .state,
                "succeeded"
            );
            assert!(
                !controller
                    .accept(&key, &run, Action::Launch, intent)
                    .unwrap()
                    .newly_accepted
            );
            let finalize = request(&project, &format!("finalize-{id}"));
            controller
                .accept(
                    &finalize,
                    &run,
                    Action::Finalize,
                    json!({"target_exited":true,"all_sources_closed":true}),
                )
                .unwrap();
            let dispatched = controller.dispatch(&finalize).unwrap();
            drop(area);
            controller.complete(&finalize,dispatched.generation,true,json!({"collection_complete":true,"temporary_inputs_removed_after_durable_extraction":true})).unwrap();

            retained.push((known_ref, known.clone()));
            retained.push((contributions.record_ref("batch"), context_record()));
            trials.push(Trial {
                run: run.id.clone(),
                value: cpu as f64,
                conditions: BTreeMap::from([
                    (
                        "inputs".into(),
                        serde_json::to_string(&input_hashes).unwrap(),
                    ),
                    ("repetitions".into(), "10000".into()),
                    ("build".into(), binary_hash.clone()),
                ]),
                complete_coverage: true,
                required_output_checks: BTreeMap::from([(
                    "all manifest entries and output bytes".into(),
                    CheckOutcome::Passed,
                )]),
            });
        }
    }
    let meaning = MeasurementMeaning {
        quantity: "process cpu".into(),
        unit: "ns".into(),
        method: "os cumulative process user+system counters between ready and held completion"
            .into(),
        population: "one target process, all its threads, excluding children".into(),
        denominator: "same directory batch and fixed repetition count".into(),
        aggregation: "user plus system delta".into(),
    };
    let baseline = Series {
        meaning: meaning.clone(),
        trials: trials[..2].to_vec(),
    };
    let changed = Series {
        meaning: meaning.clone(),
        trials: trials[2..].to_vec(),
    };
    assert_eq!(
        compare(&baseline, &changed).unwrap().status,
        ComparisonStatus::Compatible
    );
    let mut incompatible = changed.clone();
    incompatible.meaning.unit = "bytes".into();
    assert_eq!(
        compare(&baseline, &incompatible).unwrap().status,
        ComparisonStatus::Incompatible
    );
    let mut incomplete = changed.clone();
    incomplete.trials[0].complete_coverage = false;
    assert_eq!(
        compare(&baseline, &incomplete).unwrap().status,
        ComparisonStatus::Conditional
    );
    let mut failed = changed;
    failed.trials[0].required_output_checks.insert(
        "all manifest entries and output bytes".into(),
        CheckOutcome::Failed,
    );
    assert!(
        !compare(&baseline, &failed)
            .unwrap()
            .declared_output_checks_preserved
    );
    assert_eq!(
        std::fs::read_dir(project.directory().join("tmp"))
            .unwrap()
            .count(),
        0
    );
    drop(controller);
    drop(project);
    let reopened = Project::open(root.path(), "native").unwrap();
    for (reference, record) in retained {
        assert_eq!(reopened.record(&reference, None).unwrap(), record);
    }
    assert_eq!(
        reopened
            .runs(&reopened.reference("i"), QueryLimits::default(), None)
            .unwrap()
            .items
            .len(),
        4
    );
}
#[test]
#[ignore = "full native acceptance requires the selected host compiler/profiler environment; Linux requires PERFMON"]
fn c_directory_previews_use_core_control_observation_comparison_and_reopen() {
    investigation(Language::C);
}
#[test]
#[ignore = "full native acceptance requires the selected host compiler/profiler environment; Linux requires PERFMON"]
fn cpp_directory_previews_use_core_control_observation_comparison_and_reopen() {
    investigation(Language::Cpp);
}
#[test]
#[ignore = "full native acceptance requires the selected host compiler/profiler environment; macOS needs matching rust llvm-tools"]
fn rust_directory_previews_use_core_control_observation_comparison_and_reopen() {
    investigation(Language::Rust);
}

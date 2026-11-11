#[allow(dead_code)]
mod support;
use hunin_core::{
    control::{Action, Controller, RequestId},
    execution::PreparedLaunch,
    project::Project,
    records::{Record, SourceHandle},
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
    read_resources, tools,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};
fn ns(duration: Duration) -> u64 {
    duration.as_nanos().try_into().unwrap()
}
#[cfg(unix)]
fn usage() -> Value {
    let mut own: libc::rusage = unsafe { std::mem::zeroed() };
    let mut children: libc::rusage = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut own) }, 0);
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, &mut children) },
        0
    );
    let cpu = |v: &libc::rusage| {
        ((v.ru_utime.tv_sec as u64 + v.ru_stime.tv_sec as u64) * 1_000_000
            + v.ru_utime.tv_usec as u64
            + v.ru_stime.tv_usec as u64)
            * 1000
    };
    let report = json!({"self_cpu_ns":cpu(&own),"descendant_cpu_ns":cpu(&children),"peak_rss_bytes":own.ru_maxrss as u64*if cfg!(target_os="linux"){1024}else{1},"peak_rss_source":if cfg!(target_os="linux"){ "getrusage RUSAGE_SELF ru_maxrss KiB ×1024"}else{"getrusage RUSAGE_SELF ru_maxrss bytes"},"descendant_scope":"reaped direct children including the workload and profiler/export tools; not helper-exclusive cpu"});
    #[cfg(target_os = "linux")]
    {
        // ru_maxrss can retain the controller's larger pre-exec address space.
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let fields: Vec<_> = status
            .lines()
            .find(|line| line.starts_with("VmHWM:"))
            .unwrap()
            .split_whitespace()
            .collect();
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[2], "kB");
        let mut report = report;
        report["raw_getrusage_peak_rss_bytes"] = report["peak_rss_bytes"].clone();
        report["peak_rss_bytes"] = fields[1]
            .parse::<u64>()
            .unwrap()
            .checked_mul(1024)
            .unwrap()
            .into();
        report["peak_rss_source"] =
            "linux /proc/self/status VmHWM current-address-space peak KiB ×1024".into();
        report
    }
    #[cfg(not(target_os = "linux"))]
    report
}
#[cfg(windows)]
fn usage() -> Value {
    use windows_sys::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::{GetCurrentProcess, GetProcessTimes},
    };
    let handle = unsafe { GetCurrentProcess() };
    let mut created = unsafe { std::mem::zeroed() };
    let mut exited = unsafe { std::mem::zeroed() };
    let mut kernel = unsafe { std::mem::zeroed() };
    let mut user = unsafe { std::mem::zeroed() };
    assert_ne!(
        unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) },
        0
    );
    let integer = |v: windows_sys::Win32::Foundation::FILETIME| {
        ((v.dwHighDateTime as u64) << 32) | v.dwLowDateTime as u64
    };
    let mut memory: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    memory.cb = std::mem::size_of_val(&memory) as u32;
    assert_ne!(
        unsafe { GetProcessMemoryInfo(handle, &mut memory, memory.cb) },
        0
    );
    json!({"self_cpu_ns":(integer(kernel)+integer(user))*100,"descendant_cpu_ns":null,"peak_rss_bytes":memory.PeakWorkingSetSize,"peak_rss_source":"GetProcessMemoryInfo.PeakWorkingSetSize bytes","descendant_scope":"not collected; observer self and export wall time are separate"})
}
fn append(source: &SourceHandle, records: &[Record]) {
    for chunk in records.chunks(32) {
        source.append(chunk).unwrap();
    }
}
fn metric_ns(value: &Value) -> u64 {
    let start = value["start"].as_u64().unwrap();
    let end = value["end"].as_u64().unwrap();
    let frequency = value["ticks_per_second"].as_u64().unwrap();
    assert!(frequency > 0 && end >= start);
    hunin_platforms::process::Counter::new(
        end - start,
        1_000_000_000,
        frequency,
        "fixture batch clock ticks",
    )
    .unwrap()
    .nanoseconds()
    .unwrap()
}

#[test]
fn observer_worker() {
    #[cfg(target_os = "linux")]
    if let Ok(limit) = std::env::var("HUNIN_NATIVE_BENCH_MEMORY_LIMIT") {
        let memory = usage();
        assert!(memory["peak_rss_bytes"].as_u64().unwrap() < limit.parse::<u64>().unwrap());
        assert!(memory["raw_getrusage_peak_rss_bytes"].is_u64());
        return;
    }
    let Ok(configuration) = std::env::var("HUNIN_NATIVE_BENCH_WORKER") else {
        return;
    };
    let configuration: Value = serde_json::from_str(&configuration).unwrap();
    let directory = PathBuf::from(configuration["directory"].as_str().unwrap());
    let binary = PathBuf::from(configuration["binary"].as_str().unwrap());
    let language: Language = serde_json::from_value(configuration["language"].clone()).unwrap();
    let mode = configuration["mode"].as_str().unwrap();
    let code_requested = ["code-only", "combined"].contains(&mode);
    let resources_requested = ["resource-only", "combined"].contains(&mode);
    assert_eq!(sha256(&binary).unwrap(), configuration["binary_sha256"]);
    let first_usage = usage();
    let observer_start = Instant::now();
    let project = Project::create(&directory, "p").unwrap();
    let investigation = project
        .create_investigation("i", "native overhead", configuration.clone())
        .unwrap();
    let run = project
        .create_run(&investigation, "r", configuration.clone())
        .unwrap();
    let controller = Controller::new(project.clone());
    let request = RequestId {
        project: "p".into(),
        caller: "bench".into(),
        id: "launch".into(),
    };
    controller.accept(&request,&run,Action::Launch,json!({"program":binary,"sha256":configuration["binary_sha256"],"owned_scope":"process group/job"})).unwrap();
    let dispatch = controller.dispatch(&request).unwrap();
    let area = tempfile::Builder::new()
        .prefix("capture-")
        .tempdir_in(project.directory().join("tmp"))
        .unwrap();
    let output = area.path().join("output");
    std::fs::create_dir(&output).unwrap();
    let metrics = area.path().join("work.json");
    let raw = area.path().join("code.profraw");
    let launch = PreparedLaunch {
        program: binary.clone(),
        args: vec![
            configuration["input"].as_str().unwrap().into(),
            output.display().to_string(),
            "single".into(),
        ],
        directory: area.path().into(),
        stdout_limit: 65536,
        environment: BTreeMap::from([
            ("PREVIEW_REPETITIONS".into(), "10000".into()),
            ("LLVM_PROFILE_FILE".into(), raw.display().to_string()),
            ("HUNIN_WORK_METRICS".into(), metrics.display().to_string()),
        ]),
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
    let resources = resources_requested.then(|| {
        let source = project
            .register_source(
                &run,
                hunin_platforms::recording::resource_definition(
                    "resources",
                    target.clone(),
                    &first,
                ),
            )
            .unwrap();
        source
            .append(&[first.to_record("0", "target").unwrap()])
            .unwrap();
        source.ready(json!({"scope":"one process"})).unwrap();
        source
    });
    let mut resource_read_ns = 0;
    let mut resource_ingest_ns = 0;
    let mut resource_reads = 0;
    #[allow(unused_mut)]
    let mut code_drain_ns = 0;
    let code_arm_start = Instant::now();
    let mut code: Option<SourceHandle> = None;
    #[cfg(target_os = "macos")]
    let coverage = code_requested.then(|| {
        let (profdata, cov) = support::coverage_tools(language, area.path());
        let capture = CoverageBuild::bind(&binary, language, &profdata, &cov, area.path()).unwrap();
        let before = capture
            .export_owned_capture(&raw, "before", area.path())
            .unwrap();
        let source = project
            .register_source(&run, before.definition("code", target.clone()))
            .unwrap();
        append(&source, &before.records("target"));
        source
            .ready(json!({"basis":"zero counter snapshot before GO"}))
            .unwrap();
        code = Some(source);
        capture
    });
    #[cfg(target_os = "linux")]
    let mut profiler = code_requested.then(|| {
        let profiler = hunin_platforms::native::perf::PerfCollector::start(
            &instance, language, 99, 100000, 128,
        )
        .unwrap();
        let source = project
            .register_source(&run, profiler.definition("code", target.clone()))
            .unwrap();
        source
            .ready(json!({"basis":"perf enabled before GO"}))
            .unwrap();
        code = Some(source);
        profiler
    });
    #[cfg(windows)]
    let profiler = code_requested.then(|| {
        let profiler =
            hunin_platforms::native::windows::EtwRecording::start(&instance, area.path()).unwrap();
        let source = project
            .register_source(&run, profiler.definition("code", target.clone()).unwrap())
            .unwrap();
        source
            .ready(json!({"basis":"etw session enabled before GO"}))
            .unwrap();
        code = Some(source);
        profiler
    });
    let code_arm_ns = ns(code_arm_start.elapsed());
    let prepare_ns = ns(observer_start.elapsed());
    process.release().unwrap();
    let deadline = Instant::now();
    let mut max_memory: BTreeMap<String, u64> = BTreeMap::new();
    while !process.work_complete().unwrap() {
        assert!(deadline.elapsed() < Duration::from_secs(20));
        if let Some(source) = &resources {
            let start = Instant::now();
            let reading = read_resources(&instance).unwrap();
            resource_read_ns += ns(start.elapsed());
            for memory in &reading.memory {
                max_memory
                    .entry(memory.definition.clone())
                    .and_modify(|value| *value = (*value).max(memory.bytes))
                    .or_insert(memory.bytes);
            }
            let start = Instant::now();
            source
                .append(&[reading
                    .to_record(&format!("poll-{resource_reads}"), "target")
                    .unwrap()])
                .unwrap();
            resource_ingest_ns += ns(start.elapsed());
            resource_reads += 1;
        }
        #[cfg(target_os = "linux")]
        if let Some(profiler) = &mut profiler {
            let start = Instant::now();
            profiler.drain().unwrap();
            code_drain_ns += ns(start.elapsed());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let acknowledgment_ns = ns(process.acknowledged_elapsed().unwrap());
    let last = read_resources(&instance).unwrap();
    if let Some(source) = &resources {
        source
            .append(&[last.to_record("final", "target").unwrap()])
            .unwrap();
    }
    let cpu_ns = last.user_cpu.nanoseconds().unwrap() + last.system_cpu.nanoseconds().unwrap()
        - first.user_cpu.nanoseconds().unwrap()
        - first.system_cpu.nanoseconds().unwrap();
    let batch: Value = serde_json::from_slice(&std::fs::read(&metrics).unwrap()).unwrap();
    let work_ns = metric_ns(&batch);
    let finalization_start = Instant::now();
    let extract_start = Instant::now();
    let mut code_records = Vec::new();
    let mut recording_bytes = None;
    #[allow(unused_mut)]
    let mut ring_bytes: Option<u64> = None;
    #[cfg(target_os = "macos")]
    if let Some(capture) = coverage {
        recording_bytes = Some(std::fs::metadata(&raw).unwrap().len());
        code_records = capture
            .export_owned_capture(&raw, "after", area.path())
            .unwrap()
            .records("target");
    }
    #[cfg(target_os = "linux")]
    if let Some(profiler) = profiler {
        ring_bytes = Some(profiler.recording_buffer_bytes());
        let mut profile = profiler.stop().unwrap();
        hunin_platforms::native::symbols::resolve_elf_profile(&mut profile, &binary, area.path())
            .unwrap();
        recording_bytes = profile.metadata["captured_recording_bytes"].as_u64();
        code_records = profile.records("target").unwrap();
    }
    #[cfg(windows)]
    if let Some(profiler) = profiler {
        recording_bytes = Some(std::fs::metadata(&profiler.path).unwrap().len());
        let mut profile = profiler.stop(language).unwrap();
        hunin_platforms::native::windows::resolve_pdb_profile(
            &mut profile,
            &binary,
            &binary.with_extension("pdb"),
        )
        .unwrap();
        code_records = profile.records("target").unwrap();
    }
    let code_extract_ns = ns(extract_start.elapsed());
    let extracted_bytes: usize = code_records
        .iter()
        .map(|record| serde_json::to_vec(record).unwrap().len())
        .sum();
    let ingest_start = Instant::now();
    if let Some(source) = &code {
        assert!(code_records.iter().any(|record| {
            record
                .function
                .as_ref()
                .is_some_and(|name| name.contains("resize_preview"))
                && record
                    .location
                    .as_ref()
                    .is_some_and(|location| location.contains("preview."))
        }));
        append(source, &code_records);
        source.close(json!({"complete":true,"recording_bytes":recording_bytes,"extracted_json_bytes":extracted_bytes})).unwrap();
    }
    let code_ingest_ns = ns(ingest_start.elapsed());
    if let Some(source) = &resources {
        source.close(json!({"complete":true})).unwrap();
    }
    let inputs = support::assets(&directory.join("expected-inputs"));
    let checks = support::check_outputs(&output, &inputs);
    process.finish().unwrap();
    assert!(process.wait(Duration::from_secs(5)).unwrap().success);
    controller
        .complete(
            &request,
            dispatch.generation,
            true,
            json!({"output_checks":"passed"}),
        )
        .unwrap();
    drop(area);
    drop(resources);
    drop(code);
    drop(controller);
    drop(project);
    let finalization_ns = ns(finalization_start.elapsed());
    let end_to_end_ns = ns(observer_start.elapsed());
    let final_usage = usage();
    let result = json!({"mode":mode,"language":language,"build":configuration["build"],"work_ns":work_ns,"work_clock":batch,"completion_acknowledgment_ns":acknowledgment_ns,"target_cpu_ns":cpu_ns,"target_cpu_endpoint_basis":{"user_before":first.user_cpu,"system_before":first.system_cpu,"user_after":last.user_cpu,"system_after":last.system_cpu,"first_acquired":first.acquired,"last_acquired":last.acquired},"prepare_ns":prepare_ns,"finalize_ns":finalization_ns,"end_to_end_ns":end_to_end_ns,"observer_self_cpu_ns":final_usage["self_cpu_ns"].as_u64().unwrap()-first_usage["self_cpu_ns"].as_u64().unwrap(),"observer_descendant_cpu_ns":final_usage["descendant_cpu_ns"],"observer_descendant_scope":final_usage["descendant_scope"],"observer_peak_rss_bytes":final_usage["peak_rss_bytes"],"observer_peak_rss_source":final_usage["peak_rss_source"],"reference_reads":"two identical endpoint CPU/memory reads in every mode; excluded from periodic collector counters","periodic_resource_reads":resource_reads,"periodic_resource_read_ns":resource_read_ns,"resource_ingest_ns":resource_ingest_ns,"max_observed_target_memory_series_bytes":max_memory,"code_requested":code_requested,"code_arm_ns":if code_requested{Some(code_arm_ns)}else{None},"code_drain_ns":if code_requested{Some(code_drain_ns)}else{None},"code_extract_ns":if code_requested{Some(code_extract_ns)}else{None},"code_ingest_ns":if code_requested{Some(code_ingest_ns)}else{None},"recording_bytes":recording_bytes,"kernel_ring_bytes":ring_bytes,"extracted_json_bytes":if code_requested{Some(extracted_bytes)}else{None},"output_checks":checks,"peak_memory_limit":"measured fresh-observer peak; byte/sample/ring admission is enforced, no exact allocator/RSS quota is claimed"});
    #[cfg(target_os = "linux")]
    let result = {
        let mut result = result;
        result["observer_raw_getrusage_peak_rss_bytes"] =
            final_usage["raw_getrusage_peak_rss_bytes"].clone();
        result
    };
    std::fs::write(
        directory.join("result.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn observer_memory_peak_excludes_the_controllers_pre_exec_heap() {
    let held = vec![1u8; 64 * 1024 * 1024];
    let directory = tempfile::tempdir().unwrap();
    tools::require_success(
        tools::run(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "observer_worker", "--nocapture"])
                .env("HUNIN_NATIVE_BENCH_MEMORY_LIMIT", held.len().to_string()),
            directory.path(),
            Duration::from_secs(10),
            65536,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(std::hint::black_box(&held)[held.len() - 1], 1);
}

#[test]
#[ignore = "runs five fresh-process repetitions of each source mode in the selected concrete native environment"]
fn native_observation_overhead_preserves_fixed_outputs() {
    let destination = PathBuf::from(
        std::env::var("HUNIN_NATIVE_BENCH_OUTPUT").expect("set owned benchmark result directory"),
    );
    std::fs::create_dir_all(&destination).unwrap();
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("assets");
    support::assets(&input);
    let mut trials = Vec::new();
    let mut builds = Vec::new();
    for language in [Language::C, Language::Cpp, Language::Rust] {
        let plain_dir = root.path().join(format!("{language:?}-plain"));
        let code_dir = root.path().join(format!("{language:?}-code"));
        std::fs::create_dir(&plain_dir).unwrap();
        std::fs::create_dir(&code_dir).unwrap();
        let (plain, plain_compiler, plain_flags) = support::compile(language, &plain_dir, false);
        let (instrumented, code_compiler, code_flags) =
            support::compile(language, &code_dir, cfg!(target_os = "macos"));
        let plain_build = json!({"compiler":plain_compiler,"arguments":plain_flags,"sha256":sha256(&plain).unwrap(),"compiler_instrumented":false});
        let code_build = json!({"compiler":code_compiler,"arguments":code_flags,"sha256":sha256(&instrumented).unwrap(),"compiler_instrumented":cfg!(target_os="macos")});
        builds.push(json!({"language":language,"unobserved_resource_build":plain_build,"code_combined_build":code_build}));
        for repetition in 0..6 {
            let modes = if repetition % 2 == 0 {
                ["unobserved", "resource-only", "code-only", "combined"]
            } else {
                ["combined", "code-only", "resource-only", "unobserved"]
            };
            for mode in modes {
                let directory = root
                    .path()
                    .join(format!("{language:?}-{repetition}-{mode}"));
                std::fs::create_dir(&directory).unwrap();
                let (binary, build) = if ["code-only", "combined"].contains(&mode) {
                    (&instrumented, &code_build)
                } else {
                    (&plain, &plain_build)
                };
                let configuration = json!({"mode":mode,"language":language,"directory":directory,"input":input,"binary":binary,"binary_sha256":sha256(binary).unwrap(),"build":build});
                let result = tools::require_success(
                    tools::run(
                        Command::new(std::env::current_exe().unwrap())
                            .args(["--exact", "observer_worker", "--nocapture"])
                            .env(
                                "HUNIN_NATIVE_BENCH_WORKER",
                                serde_json::to_string(&configuration).unwrap(),
                            ),
                        &directory,
                        Duration::from_secs(30),
                        1024 * 1024,
                    )
                    .unwrap(),
                )
                .unwrap();
                let _ = result;
                let mut value: Value =
                    serde_json::from_slice(&std::fs::read(directory.join("result.json")).unwrap())
                        .unwrap();
                value["repetition"] = repetition.into();
                value["warmup"] = (repetition == 0).into();
                assert_eq!(value["output_checks"]["outcome"], "passed");
                trials.push(value);
            }
        }
    }
    let host = if cfg!(unix) {
        support::checked(Command::new("uname").arg("-a"), root.path())
    } else {
        "windows execution requires a separately pinned host receipt".into()
    };
    let result = json!({"host":host,"architecture":std::env::consts::ARCH,"trials":trials,"builds":builds,"method":{"warmups":"one complete repetition per language/mode excluded from descriptive summaries","repetitions":5,"ordering":"forward/reverse alternating mode order","fixed_input":"three deterministic P6 assets, nearest-neighbor32x32,10000 repeats per asset","cpu":"target endpoints measured in every mode; periodic collector reads timed separately","memory":if cfg!(target_os="linux"){ "fresh observer /proc/self/status VmHWM current-address-space peak; raw getrusage peak retained separately and may include pre-exec high water" }else{"fresh observer subprocess ru_maxrss/PeakWorkingSet; no cross-trial inherited high water mark"},"claim_ceiling":"descriptive individual trials; no universal overhead percentage, speedup or hard heap/RSS cap"}});
    std::fs::write(
        destination.join("results.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}

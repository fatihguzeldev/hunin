mod paired;

use hunin_runtimes::{Limits, clr};
use serde_json::{Value, json};

fn profile() -> Value {
    json!({"format":"hunin-eventpipe-v1","traceevent":"4.0.0","runtime":"10.0.15","frequency":10000000,"origin_qpc":50,"events_lost":0,"source_binding":{"matched_pdb":true},"warnings":[],"future_header":{"retained":true},"frames":[{"id":1,"address":u64::MAX,"function":"Preview.Resize","module":"preview.dll","module_pdb":null,"token":100663297,"il_offset":12,"source_file":"Preview.cs","source_line":8,"source_mapping":"matched portable-pdb il sequence point","future_frame":42}],"stacks":[{"id":0,"frame":1,"caller":null}],"events":[{"id":7,"provider":"Microsoft-DotNETCore-SampleProfiler","name":"Thread/Sample","provider_guid":"00000000-0000-0000-0000-000000000001","event_id":1,"version":0,"activity":"00000000-0000-0000-0000-000000000000","related_activity":"00000000-0000-0000-0000-000000000000","process":42,"thread":43,"time_qpc":51,"time_ns":100,"uncertainty_ns":100,"stack":0,"payload":{"counter":u64::MAX},"future_event":"retained"}]})
}

#[test]
fn eventpipe_retains_exact_ticks_frames_stack_edges_payload_and_loss() {
    let mut profile = profile();
    profile["events_lost"] = json!(9);
    let extracted = clr::extract(
        &serde_json::to_vec(&profile).unwrap(),
        "clr",
        Limits::default(),
    )
    .unwrap();
    let event = extracted
        .records
        .iter()
        .find(|record| record.schema == "eventpipe.event")
        .unwrap();
    assert_eq!(event.payload["payload"]["counter"], json!(u64::MAX));
    assert_eq!(event.payload["future_event"], "retained");
    let time = event.time.as_ref().unwrap();
    assert_eq!((time.start, time.uncertainty), (100, 100));
    assert_eq!(event.function.as_deref(), Some("Preview.Resize"));
    assert_eq!(event.location.as_deref(), Some("Preview.cs:8"));
    let frame = extracted
        .records
        .iter()
        .find(|record| record.schema == "eventpipe.frame")
        .unwrap();
    assert_eq!(frame.payload["address"], json!(u64::MAX));
    assert_eq!(frame.payload["future_frame"], 42);
    assert!(!extracted.warnings.is_empty());
    assert_eq!(
        extracted.records.last().unwrap().payload["future_header"]["retained"],
        true
    );
}

#[test]
fn eventpipe_rejects_mistaken_clocks_cycles_references_and_pressure() {
    let mut cases = Vec::new();
    let mut wrong = profile();
    wrong["events"][0]["time_ns"] = json!(99);
    cases.push(wrong);
    let mut wrong = profile();
    wrong["frequency"] = json!(0);
    cases.push(wrong);
    let mut wrong = profile();
    wrong["events"][0]["uncertainty_ns"] = json!(1);
    cases.push(wrong);
    let mut wrong = profile();
    wrong["stacks"][0]["caller"] = json!(0);
    cases.push(wrong);
    let mut wrong = profile();
    wrong["stacks"][0]["frame"] = json!(8);
    cases.push(wrong);
    let mut wrong = profile();
    wrong["events"][0]["stack"] = json!(8);
    cases.push(wrong);
    let mut wrong = profile();
    wrong["frames"][0]["source_line"] = Value::Null;
    cases.push(wrong);
    for wrong in cases {
        assert!(
            clr::extract(
                &serde_json::to_vec(&wrong).unwrap(),
                "clr",
                Limits::default()
            )
            .is_err()
        );
    }
    for limits in [
        Limits {
            max_input_bytes: 8,
            ..Limits::default()
        },
        Limits {
            max_records: 1,
            ..Limits::default()
        },
        Limits {
            max_output_bytes: 8,
            ..Limits::default()
        },
    ] {
        assert!(clr::extract(&serde_json::to_vec(&profile()).unwrap(), "clr", limits).is_err());
    }
}

#[test]
fn unresolved_eventpipe_symbols_keep_runtime_names_without_source_locations() {
    let mut profile = profile();
    profile["frames"][0]["source_file"] = Value::Null;
    profile["frames"][0]["source_line"] = Value::Null;
    profile["frames"][0]["source_mapping"] = json!("source checksum mismatch");
    let extraction = clr::extract(
        &serde_json::to_vec(&profile).unwrap(),
        "clr",
        Limits::default(),
    )
    .unwrap();
    let frame = extraction
        .records
        .iter()
        .find(|record| record.schema == "eventpipe.frame")
        .unwrap();
    assert_eq!(frame.function.as_deref(), Some("Preview.Resize"));
    assert!(frame.location.is_none());
}

#[test]
#[cfg(unix)]
fn coreclr_long_project_paths_fail_before_compilation_or_file_writes() {
    use std::{fs, path::PathBuf};
    let directory = tempfile::tempdir().unwrap();
    let area = directory
        .path()
        .join("long-project-temporary-directory-that-cannot-fit-a-diagnostic-socket");
    fs::create_dir(&area).unwrap();
    let executable = std::env::current_exe().unwrap();
    let workload = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Preview.cs");
    let error = clr::prepare(&executable, &workload, &area).unwrap_err();
    assert!(error.to_string().contains("shorter project data root"));
    assert_eq!(fs::read_dir(&area).unwrap().count(), 0);
}

#[test]
#[ignore = "requires HUNIN_DOTNET SDK10.0.404/runtime10.0.15 and a short HUNIN_CLR_DATA_ROOT"]
fn real_coreclr_samples_match_portable_pdb_and_survive_input_removal() {
    use hunin_core::{
        execution::PreparedProcess,
        project::Project,
        query::{Query, QueryLimits},
    };
    use std::{fs, path::PathBuf, time::Duration};
    let executable = PathBuf::from(std::env::var_os("HUNIN_DOTNET").expect("set HUNIN_DOTNET"));
    let root = PathBuf::from(
        std::env::var_os("HUNIN_CLR_DATA_ROOT").expect("set a short HUNIN_CLR_DATA_ROOT"),
    );
    let id = Project::new_id().unwrap()[..3].to_owned();
    let project = Project::create(&root, &id).unwrap();
    let investigation = project
        .create_investigation("i", "clr preview", json!({}))
        .unwrap();
    let run = project
        .create_run(
            &investigation,
            "r",
            json!({"workload":"seven pixel previews"}),
        )
        .unwrap();
    let area = project.directory().join(if cfg!(windows) {
        "tmp/long-project-temporary-directory-with-windows-named-pipes"
    } else {
        "tmp/c"
    });
    fs::create_dir(&area).unwrap();
    let workload = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Preview.cs");
    let mut prepared = clr::prepare(&executable, &workload, &area).unwrap();
    prepared
        .launch
        .environment
        .insert("HUNIN_FINALIZE_GATE".into(), "1".into());
    let mut process = PreparedProcess::spawn(&prepared.launch).unwrap();
    process.await_ready(Duration::from_secs(30)).unwrap();
    let mut resources = paired::Resources::start(&project, &run, process.id());
    let target = resources.target.clone();
    let source = project
        .register_source(&run, prepared.definition("eventpipe", &target))
        .unwrap();
    source
        .ready(json!({"session":"acknowledged before gate","runtime_startup":"excluded"}))
        .unwrap();
    process.release().unwrap();
    let (resource_count, cpu_ns) = resources.finish_work(&mut process);
    let result = process.wait(Duration::from_secs(30)).unwrap();
    assert!(result.success, "{result:?}");
    let extraction = prepared.extract("clr-process", Limits::default()).unwrap();
    let report = extraction
        .records
        .iter()
        .find(|record| record.schema == "prepared.report")
        .unwrap();
    assert_eq!(report.payload["runtime"]["clr"], "10.0.15");
    for (index, pixel) in report.payload["output"]["pixels"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        assert_eq!(
            pixel.as_u64().unwrap(),
            (((index * 3 + 7) ^ index) & 255) as u64
        );
    }
    let header = extraction
        .records
        .iter()
        .find(|record| record.schema == "eventpipe.profile")
        .unwrap();
    assert_eq!(header.payload["events_lost"], 0);
    assert_eq!(header.payload["source_binding"]["matched_pdb"], true);
    let frame = extraction
        .records
        .iter()
        .find(|record| {
            record.schema == "eventpipe.frame"
                && record
                    .function
                    .as_ref()
                    .is_some_and(|function| function.contains("Preview.Resize"))
                && record
                    .location
                    .as_ref()
                    .is_some_and(|location| location.contains("Preview.cs:"))
        })
        .expect("known workload frame must have a matched source location");
    let function = frame.function.clone().unwrap();
    let frames = extraction
        .records
        .iter()
        .filter(|record| record.schema == "eventpipe.frame")
        .map(|record| {
            (
                record.payload["id"].as_u64().unwrap(),
                record
                    .function
                    .as_ref()
                    .is_some_and(|name| name.contains("Preview.Resize")),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let stacks = extraction
        .records
        .iter()
        .filter(|record| record.schema == "eventpipe.stack")
        .map(|record| {
            (
                record.payload["id"].as_u64().unwrap(),
                (
                    record.payload["frame"].as_u64().unwrap(),
                    record.payload["caller"].as_u64(),
                ),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        header.payload["frequency"],
        report.payload["helper_frequency"]
    );
    let work_start = report.payload["work_start_qpc"].as_i64().unwrap();
    let work_end = report.payload["work_end_qpc"].as_i64().unwrap();
    let mut workload_samples = 0;
    for event in extraction.records.iter().filter(|record| {
        record.schema == "eventpipe.event"
            && record.payload["provider"] == "Microsoft-DotNETCore-SampleProfiler"
    }) {
        let mut stack = event.payload["stack"].as_u64();
        let mut observed = false;
        while let Some(id) = stack {
            let (frame, caller) = stacks[&id];
            observed |= frames[&frame];
            stack = caller;
        }
        if observed {
            let timestamp = event.payload["time_qpc"].as_i64().unwrap();
            assert!((work_start..=work_end).contains(&timestamp));
            workload_samples += 1;
        }
    }
    assert!(
        workload_samples > 0,
        "known workload must appear on an actual sample stack inside the independent work interval"
    );
    for batch in extraction.records.chunks(256) {
        source.append(batch).unwrap();
    }
    source
        .close(
            json!({"complete":extraction.warnings.is_empty(),"records":extraction.records.len()}),
        )
        .unwrap();
    fs::remove_dir_all(&area).unwrap();
    drop(source);
    drop(resources);
    drop(project);
    let reopened = Project::open(&root, &id).unwrap();
    paired::verify_reopened(&reopened, &run, resource_count);
    eprintln!(
        "clr-paired resource_records={resource_count} cpu_ns={cpu_ns} runtime_records={} workload_samples={workload_samples} platform={}",
        extraction.records.len(),
        std::env::consts::OS
    );
    let mut query = Query::new(run);
    query.source = Some("eventpipe".into());
    query.schema = Some("eventpipe.frame".into());
    query.function = Some(function);
    let page = reopened
        .query(&query, QueryLimits::default(), None)
        .unwrap();
    assert!(page.records.iter().any(|entry| {
        entry
            .record
            .as_ref()
            .unwrap()
            .location
            .as_ref()
            .is_some_and(|location| location.contains("Preview.cs:"))
    }));
    let directory = reopened.directory().to_owned();
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
#[ignore = "requires selected HUNIN_DOTNET and a short HUNIN_CLR_DATA_ROOT"]
fn real_coreclr_rejects_missing_foreign_pdb_and_changed_source_associations() {
    use hunin_core::{execution::PreparedProcess, project::Project};
    use std::{fs, path::PathBuf, time::Duration};
    let executable = PathBuf::from(std::env::var_os("HUNIN_DOTNET").expect("set HUNIN_DOTNET"));
    let root = PathBuf::from(
        std::env::var_os("HUNIN_CLR_DATA_ROOT").expect("set short HUNIN_CLR_DATA_ROOT"),
    );
    let id = Project::new_id().unwrap()[..3].to_owned();
    let project = Project::create(&root, &id).unwrap();
    let area = project.directory().join("tmp/c");
    fs::create_dir(&area).unwrap();
    let workload = project.directory().join("Preview.cs");
    let original = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Preview.cs"),
    )
    .unwrap();
    fs::write(&workload, &original).unwrap();
    let prepared = clr::prepare(&executable, &workload, &area).unwrap();
    let pdb = area.join("bin/Release/net10.0/HuninTrace.pdb");
    let original_pdb = fs::read(&pdb).unwrap();
    let run = |expected: &str| {
        for name in [
            "runtime.nettrace",
            "runtime.etlx",
            "runtime-profile.json",
            "runtime-report.json",
        ] {
            let path = area.join(name);
            if path.exists() {
                fs::remove_file(path).unwrap();
            }
        }
        let mut process = PreparedProcess::spawn(&prepared.launch).unwrap();
        process.await_ready(Duration::from_secs(30)).unwrap();
        process.release().unwrap();
        let result = process.wait(Duration::from_secs(30)).unwrap();
        assert!(result.success, "{result:?}");
        let extraction = prepared.extract("clr", Limits::default()).unwrap();
        assert!(!extraction.warnings.is_empty());
        assert!(
            extraction
                .warnings
                .iter()
                .any(|warning| warning.contains(expected)),
            "{:?}",
            extraction.warnings
        );
        let frames = extraction
            .records
            .iter()
            .filter(|record| {
                record.schema == "eventpipe.frame"
                    && record
                        .function
                        .as_ref()
                        .is_some_and(|name| name.contains("Preview.Resize"))
            })
            .collect::<Vec<_>>();
        assert!(!frames.is_empty());
        assert!(frames.iter().all(|frame| frame.location.is_none()));
    };
    fs::remove_file(&pdb).unwrap();
    run("source binding unavailable");
    let foreign_area = project.directory().join("tmp/d");
    fs::create_dir(&foreign_area).unwrap();
    let foreign_source = foreign_area.join("Preview.cs");
    fs::write(&foreign_source, original.replace("150000", "150001")).unwrap();
    let _foreign = clr::prepare(&executable, &foreign_source, &foreign_area).unwrap();
    fs::copy(
        foreign_area.join("bin/Release/net10.0/HuninTrace.pdb"),
        &pdb,
    )
    .unwrap();
    run("portable pdb identity differs");
    fs::write(&pdb, original_pdb).unwrap();
    fs::write(&workload, original.replace("150000", "150002")).unwrap();
    run("source checksum mismatch");
    let directory = project.directory().to_owned();
    drop(project);
    fs::remove_dir_all(directory).unwrap();
}

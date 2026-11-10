mod paired;

use hunin_runtimes::{Limits, php};
use serde_json::json;

const PROFILE: &str = "version: 1\ncreator: xdebug 3.5.3 (PHP 8.5.4)\ncmd: fixture.php\npart: 1\npositions: line\nevents: Time_(10ns) Memory_(bytes)\n\nfl=(2) /fixture/a: b.php\nfn=(1) resize\n4 18446744073709551615 8\n\nfl=(2)\nfn=(2) run\n14 30 4\ncfl=(2)\ncfn=(1)\ncalls=7 0 0\n18 120 32\n\nsummary: 150 2048\n";

#[test]
fn xdebug_keeps_exact_self_costs_compressed_names_and_call_counts() {
    let extracted = php::extract(PROFILE.as_bytes(), "php-process", Limits::default()).unwrap();
    assert!(extracted.clocks.is_empty());
    assert!(extracted.warnings.is_empty());
    let cost = extracted
        .records
        .iter()
        .find(|record| {
            record.schema == "xdebug.cost" && record.function.as_deref() == Some("resize")
        })
        .unwrap();
    assert_eq!(cost.payload["time_10ns"], json!(u64::MAX));
    assert_eq!(cost.payload["file"], "/fixture/a: b.php");
    assert_eq!(cost.location.as_deref(), Some("/fixture/a: b.php:4"));
    assert!(cost.time.is_none());
    let call = extracted
        .records
        .iter()
        .find(|record| record.schema == "xdebug.call")
        .unwrap();
    assert_eq!(call.payload["calls"], 7);
    assert_eq!(call.payload["caller_function"], "run");
    assert_eq!(call.payload["callee_function"], "resize");
    assert_eq!(call.payload["target_positions"], json!([0, 0]));
    assert_eq!(call.payload["time_10ns"], 120);
    let caller = extracted
        .records
        .iter()
        .find(|record| record.id == call.payload["caller_cost"])
        .unwrap();
    assert_eq!(caller.function.as_deref(), Some("run"));
    let header = extracted
        .records
        .iter()
        .find(|record| record.schema == "xdebug.profile")
        .unwrap();
    assert_eq!(header.payload["summary"], json!([150, 2048]));
    assert_eq!(header.payload["complete"], true);
    assert_eq!(
        extracted
            .schemas
            .iter()
            .find(|schema| schema.name == "xdebug.cost")
            .unwrap()
            .fields["time_10ns"]
            .unit
            .as_deref(),
        Some("10ns")
    );
}

#[test]
fn xdebug_truncation_keeps_a_marked_prefix_and_incompatible_units_fail() {
    let prefix = PROFILE.split("summary:").next().unwrap();
    let extracted = php::extract(prefix.as_bytes(), "php", Limits::default()).unwrap();
    assert!(!extracted.warnings.is_empty());
    assert!(
        extracted
            .records
            .iter()
            .any(|record| record.schema == "xdebug.cost")
    );
    assert_eq!(extracted.records.last().unwrap().payload["complete"], false);
    for wrong in [
        PROFILE.replace("Time_(10ns)", "Time_(seconds)"),
        PROFILE.replace("fl=(2) /fixture/a: b.php", "fl=(2)"),
        PROFILE.replace("fn=(2) run", "fn=(1) different"),
        PROFILE.replace("positions: line", "positions: instr line"),
        PROFILE.replace("3.5.3", "3.5.4"),
        PROFILE.replace("calls=7 0 0", "calls=18446744073709551616 0 0"),
    ] {
        assert!(php::extract(wrong.as_bytes(), "php", Limits::default()).is_err());
    }
    let pending = format!("{}cfl=(2)\ncfn=(1)\ncalls=1 0 0\n", prefix);
    let extraction = php::extract(pending.as_bytes(), "php", Limits::default()).unwrap();
    assert_eq!(
        extraction.records.last().unwrap().payload["complete"],
        false
    );
}

#[test]
fn xdebug_rejects_input_output_record_and_string_pressure() {
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
        Limits {
            max_string_bytes: 8,
            ..Limits::default()
        },
    ] {
        assert!(php::extract(PROFILE.as_bytes(), "php", limits).is_err());
    }
}

#[test]
#[ignore = "requires absolute HUNIN_PHP and HUNIN_XDEBUG matching PHP8.5.4/Xdebug3.5.3"]
fn real_php_records_known_calls_and_reopens_without_the_cachegrind_input() {
    use hunin_core::{
        execution::PreparedProcess,
        project::Project,
        query::{FieldFilter, Query, QueryLimits},
    };
    use std::{fs, path::PathBuf, time::Duration};
    let executable = PathBuf::from(std::env::var_os("HUNIN_PHP").expect("set HUNIN_PHP"));
    let extension = PathBuf::from(std::env::var_os("HUNIN_XDEBUG").expect("set HUNIN_XDEBUG"));
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "php").unwrap();
    let investigation = project
        .create_investigation("i", "php preview", json!({}))
        .unwrap();
    let run = project
        .create_run(&investigation, "r", json!({"fixture":"pixel preview"}))
        .unwrap();
    let area = project.directory().join("tmp/php");
    fs::create_dir(&area).unwrap();
    let workload = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/preview.php");
    let mut prepared = php::prepare(&executable, &extension, &workload, &area).unwrap();
    prepared
        .launch
        .environment
        .insert("HUNIN_FINALIZE_GATE".into(), "1".into());
    let mut process = PreparedProcess::spawn(&prepared.launch).unwrap();
    process.await_ready(Duration::from_secs(10)).unwrap();
    let mut resources = paired::Resources::start(&project, &run, process.id());
    let target = resources.target.clone();
    let source = project
        .register_source(&run, prepared.definition("xdebug", &target))
        .unwrap();
    source
        .ready(json!({"active_profile_verified":true,"scope":"owned process startup through exit"}))
        .unwrap();
    process.release().unwrap();
    let (resource_count, cpu_ns) = resources.finish_work(&mut process);
    let outcome = process.wait(Duration::from_secs(30)).unwrap();
    assert!(outcome.success, "{outcome:?}");
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(&prepared.report_path).unwrap()).unwrap();
    assert_eq!(report["runtime"]["php"], "8.5.4");
    assert_eq!(report["runtime"]["xdebug"], "3.5.3");
    assert_eq!(report["outcome"], "completed");
    for (index, pixel) in report["output"]["pixels"]
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
    let extraction = prepared.extract("php-process", Limits::default()).unwrap();
    assert!(extraction.warnings.is_empty());
    let expected = extraction.records.len();
    assert_eq!(
        extraction
            .records
            .iter()
            .filter(|record| record.schema == "xdebug.cost"
                && record.function.as_deref() == Some("resize"))
            .count(),
        7
    );
    assert_eq!(
        extraction
            .records
            .iter()
            .filter(|record| record.schema == "xdebug.call"
                && record.payload["callee_function"] == "resize")
            .map(|record| record.payload["calls"].as_u64().unwrap())
            .sum::<u64>(),
        7
    );
    for batch in extraction.records.chunks(256) {
        source.append(batch).unwrap();
    }
    source
        .close(json!({"complete":true,"records":expected}))
        .unwrap();
    fs::remove_dir_all(&area).unwrap();
    drop(source);
    drop(resources);
    drop(project);
    let reopened = Project::open(directory.path(), "php").unwrap();
    let discovered = reopened
        .sources(&run, QueryLimits::default(), None)
        .unwrap();
    assert!(
        discovered
            .items
            .iter()
            .any(|source| source.id == "xdebug" && source.closed)
    );
    paired::verify_reopened(&reopened, &run, resource_count);
    eprintln!(
        "php-paired resource_records={resource_count} cpu_ns={cpu_ns} runtime_records={expected} platform={}",
        std::env::consts::OS
    );
    let mut query = Query::new(run);
    query.source = Some("xdebug".into());
    query.schema = Some("xdebug.cost".into());
    query.fields.push(FieldFilter {
        name: "function".into(),
        value: json!("resize"),
    });
    let page = reopened
        .query(&query, QueryLimits::default(), None)
        .unwrap();
    assert_eq!(page.records.len(), 7);
    assert!(page.records.iter().all(|entry| {
        entry.record.as_ref().unwrap().payload["time_10ns"]
            .as_u64()
            .unwrap()
            > 0
    }));
}

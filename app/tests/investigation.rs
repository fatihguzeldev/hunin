use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use hunin_app::{BeginCapture, CapturePlan};
use hunin_core::compare::{
    CheckOutcome, ComparisonStatus, MeasurementMeaning, Series, Trial, compare,
};
use hunin_core::control::RequestId;
use hunin_core::project::Project;
use hunin_core::query::{Context, Origin, Query, QueryLimits};
use hunin_runtimes::RuntimeKind;
use serde_json::json;

fn run_investigation(kind: RuntimeKind, variable: &str, fixture: &str) {
    let executable =
        PathBuf::from(std::env::var(variable).expect("set selected absolute runtime executable"));
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let workload = workspace.join("runtimes/tests/fixtures").join(fixture);
    let area = tempfile::tempdir().unwrap();
    let project = Project::create(area.path(), "preview-project").unwrap();
    let investigation = project
        .create_investigation(
            "i",
            "preview batch",
            json!({"input":"independently generated pixels"}),
        )
        .unwrap();
    let mut observed = Vec::new();
    let mut retained = Vec::new();
    for repetition in 0..4 {
        let run = project
            .create_run(
                &investigation,
                &format!("run-{repetition}"),
                json!({"input":"fixed pixel vector","runtime":format!("{kind:?}")}),
            )
            .unwrap();
        let request = RequestId {
            project: project.id().into(),
            caller: "integration-agent".into(),
            id: format!("capture-{repetition}"),
        };
        let plan = || CapturePlan {
            request: request.clone(),
            run: run.clone(),
            runtime: kind,
            executable: executable.clone(),
            workload: workload.clone(),
            conditions: json!({"input":"fixed pixel vector"}),
            environment: BTreeMap::new(),
            read_paths: vec![],
            write_paths: vec![],
            xdebug_extension: None,
        };
        let BeginCapture::Started(mut capture) = hunin_app::begin(project.clone(), plan()).unwrap()
        else {
            panic!("fresh request must launch")
        };
        assert!(
            !project
                .query(&Query::new(run.clone()), QueryLimits::default(), None)
                .unwrap()
                .records
                .is_empty(),
            "initial evidence is durable before work"
        );
        project.add_context(&run,Context{id:"asset-selection".into(),subject:"preview-operation".into(),origin:Origin::Contributed,value:json!({"asset":"asset-1","description":"agent supplied application meaning"}),corrects:None}).unwrap();
        capture.release().unwrap();
        let report = capture.finish(Duration::from_secs(30)).unwrap();
        assert_eq!(report.application["outcome"], "completed");
        let pixels = report.application["output"]["pixels"].as_array().unwrap();
        assert!(!pixels.is_empty());
        for (i, pixel) in pixels.iter().enumerate() {
            assert_eq!(
                pixel.as_u64().unwrap(),
                ((((i & 255) * 3 + 7) ^ (i & 255)) & 255) as u64
            );
        }
        project.add_context(&run,Context{id:"output-check".into(),subject:"preview-operation".into(),origin:Origin::Contributed,value:json!({"scope":"every output pixel","outcome":"passed","pixels":pixels.len()}),corrects:None}).unwrap();
        assert!(report.cpu_ns.parse::<u64>().unwrap() > 0);
        assert!(report.resource_observations >= 2);
        let mut query = Query::new(run.clone());
        query.function = Some("resize".into());
        query.source = Some(report.code_sources[0].clone());
        let found = project.query(&query, QueryLimits::default(), None).unwrap();
        assert!(
            !found.records.is_empty(),
            "automatic function evidence is independent of operation markers"
        );
        let record = project.record(&found.records[0].reference, None).unwrap();
        assert!(
            record
                .location
                .as_ref()
                .is_some_and(|path| path.contains(fixture))
        );
        retained.push((found.records[0].reference.clone(), record));
        assert!(
            matches!(
                hunin_app::begin(project.clone(), plan()).unwrap(),
                BeginCapture::Existing(_)
            ),
            "lost response retry cannot launch again"
        );
        observed.push(Trial {
            run: run.id,
            value: report.cpu_ns.parse().unwrap(),
            conditions: BTreeMap::from([("input".into(), "fixed pixel vector".into())]),
            complete_coverage: true,
            required_output_checks: BTreeMap::from([(
                "every output pixel".into(),
                CheckOutcome::Passed,
            )]),
        });
    }
    let meaning = MeasurementMeaning {
        quantity: "cpu time".into(),
        unit: "ns".into(),
        method: "process counter delta from readiness to held completion".into(),
        population: "one prepared process".into(),
        denominator: "one fixed preview batch".into(),
        aggregation: "user plus system".into(),
    };
    let a = Series {
        meaning: meaning.clone(),
        trials: observed[..2].to_vec(),
    };
    let b = Series {
        meaning,
        trials: observed[2..].to_vec(),
    };
    assert_eq!(
        compare(&a, &b).unwrap().status,
        ComparisonStatus::Compatible
    );
    assert_eq!(
        std::fs::read_dir(project.directory().join("tmp"))
            .unwrap()
            .count(),
        0,
        "no original profiler containers remain"
    );
    drop(project);
    let reopened = Project::open(area.path(), "preview-project").unwrap();
    for (reference, expected) in retained {
        assert_eq!(reopened.record(&reference, None).unwrap(), expected);
    }
}

#[test]
#[ignore = "requires selected HUNIN_NODE runtime"]
fn node_preview_repeats_queries_and_reopens_without_recording_files() {
    run_investigation(RuntimeKind::Node, "HUNIN_NODE", "preview.cjs");
}

#[test]
#[ignore = "requires selected HUNIN_PYTHON runtime"]
fn python_preview_repeats_queries_and_reopens_without_recording_files() {
    run_investigation(RuntimeKind::CPython, "HUNIN_PYTHON", "preview.py");
}

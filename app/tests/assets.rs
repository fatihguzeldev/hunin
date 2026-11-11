use hunin_app::{BeginCapture, CapturePlan};
use hunin_core::{
    compare::{CheckOutcome, ComparisonStatus, MeasurementMeaning, Series, Trial, compare},
    control::RequestId,
    project::Project,
    query::{Context, Direction, FieldFilter, Origin, Query, QueryLimits, Relationship},
    records::{FieldSchema, FieldType, Record, RecordSchema, SourceDefinition},
};
use hunin_runtimes::RuntimeKind;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::PathBuf, time::Duration};

fn asset_investigation(kind: RuntimeKind, variable: &str, fixture: &str, repetitions: usize) {
    let executable = PathBuf::from(std::env::var_os(variable).expect("set selected runtime path"));
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let workload = workspace.join("runtimes/tests/fixtures").join(fixture);
    let data = if cfg!(unix) {
        tempfile::Builder::new()
            .prefix("h")
            .rand_bytes(3)
            .tempdir_in("/tmp")
            .unwrap()
    } else {
        tempfile::tempdir().unwrap()
    };
    let project = Project::create(data.path(), "p").unwrap();
    let investigation = project
        .create_investigation(
            "i",
            "asset previews",
            json!({"caller_defined_algorithm":"nearest-neighbor-rgb8"}),
        )
        .unwrap();
    let input = project.directory().join("fixture-input");
    fs::create_dir(&input).unwrap();
    let mut expected = Vec::new();
    for asset in 0..2 {
        let name = format!("asset-{asset}.ppm");
        let mut source = b"P6\n64 64\n255\n".to_vec();
        let pixels: Vec<u8> = (0..64 * 64 * 3)
            .map(|index| ((index * 17 + asset * 37 + index / 13) & 255) as u8)
            .collect();
        source.extend(&pixels);
        fs::write(input.join(&name), &source).unwrap();
        let mut preview = b"P6\n32 32\n255\n".to_vec();
        for y in 0..32 {
            for x in 0..32 {
                let offset = ((y * 2) * 64 + x * 2) * 3;
                preview.extend_from_slice(&pixels[offset..offset + 3]);
            }
        }
        expected.push((name, format!("asset-{asset}.preview.ppm"), preview));
    }
    let input_identity = format!(
        "{:x}",
        Sha256::digest(
            expected
                .iter()
                .flat_map(|(name, _, _)| fs::read(input.join(name)).unwrap())
                .collect::<Vec<_>>()
        )
    );
    let mut trials = Vec::new();
    let mut retained = Vec::new();
    for variant in 0..2 {
        for repetition in 0..2 {
            let count = repetitions * (variant + 1);
            let run=project.create_run(&investigation,&format!("variant-{variant}-{repetition}"),json!({"input":input_identity,"variant":variant,"repetition":repetition,"configured_repeated_work":count,"runtime":format!("{kind:?}")})).unwrap();
            let output = project
                .directory()
                .join(format!("output-{variant}-{repetition}"));
            fs::create_dir(&output).unwrap();
            let request = RequestId {
                project: "p".into(),
                caller: "asset-agent".into(),
                id: run.id.clone(),
            };
            let environment = BTreeMap::from([
                ("PREVIEW_INPUT".into(), input.to_string_lossy().into_owned()),
                (
                    "PREVIEW_OUTPUT".into(),
                    output.to_string_lossy().into_owned(),
                ),
                ("PREVIEW_REPETITIONS".into(), count.to_string()),
            ]);
            let plan = || CapturePlan {
                request: request.clone(),
                run: run.clone(),
                runtime: kind,
                executable: executable.clone(),
                workload: workload.clone(),
                conditions: json!({"input":input_identity,"algorithm":"nearest-neighbor-rgb8","configured_repeated_work":count}),
                environment: environment.clone(),
                read_paths: vec![input.clone()],
                write_paths: vec![output.clone()],
                xdebug_extension: if kind == RuntimeKind::PHP {
                    Some(PathBuf::from(
                        std::env::var_os("HUNIN_XDEBUG").expect("set matching Xdebug extension"),
                    ))
                } else {
                    None
                },
            };
            let BeginCapture::Started(mut capture) =
                hunin_app::begin(project.clone(), plan()).unwrap()
            else {
                panic!("fresh request")
            };
            project.add_context(&run,Context{id:"operation".into(),subject:"preview-operation".into(),origin:Origin::Contributed,value:json!({"input":input_identity,"assets":["asset-0.ppm","asset-1.ppm"],"meaning":"caller selected asset batch; no process CPU ownership inferred"}),corrects:None}).unwrap();
            capture.release().unwrap();
            let report = capture.finish(Duration::from_secs(90)).unwrap();
            assert_eq!(report.application["outcome"], "completed");
            assert_eq!(report.request.state, "succeeded", "{:?}", report.warnings);
            let manifest: Value =
                serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
            assert_eq!(manifest["algorithm"], "nearest-neighbor-rgb8");
            assert_eq!(manifest["width"], 32);
            assert_eq!(manifest["height"], 32);
            let assets = manifest["assets"].as_array().unwrap();
            assert_eq!(assets.len(), expected.len());
            assert_eq!(manifest, report.application["output"]);
            let driver=project.register_source(&run,SourceDefinition{id:"output-measurements".into(),target:None,schemas:vec![RecordSchema{name:"driver.asset-output".into(),version:1,description:"caller independently reads and checks each produced image".into(),fields:BTreeMap::from([("bytes".into(),FieldSchema{kind:FieldType::Unsigned,required:true,unit:Some("bytes".into()),meaning:"actual output file length".into()}),("sha256".into(),FieldSchema{kind:FieldType::String,required:true,unit:None,meaning:"sha256 of actual output bytes".into()}),("input".into(),FieldSchema{kind:FieldType::String,required:true,unit:None,meaning:"caller input selection".into()})])}],clocks:vec![],interpretation:"independent-output-oracle-v1".into(),metadata:json!({"scope":"post-execution filesystem reads","origin":"caller driver","cpu_attribution":"none"})}).unwrap();
            for (index, (name, destination, bytes)) in expected.iter().enumerate() {
                let actual = fs::read(output.join(destination)).unwrap();
                assert_eq!(&actual, bytes, "{kind:?} output pixel mismatch");
                let digest = format!("{:x}", Sha256::digest(&actual));
                assert_eq!(
                    assets[index],
                    json!({"input":name,"output":destination,"source_width":64,"source_height":64,"bytes":bytes.len(),"sha256":digest})
                );
                driver
                    .append(&[Record {
                        id: index.to_string(),
                        schema: "driver.asset-output".into(),
                        schema_version: 1,
                        subject: destination.clone(),
                        time: None,
                        function: None,
                        location: None,
                        operation: Some("preview-operation".into()),
                        payload: json!({"bytes":actual.len(),"sha256":digest,"input":name}),
                    }])
                    .unwrap();
            }
            driver
                .close(json!({"complete":true,"output_check":"every output byte plus manifest"}))
                .unwrap();
            let mut query = Query::new(run.clone());
            query.source = Some(report.code_sources[0].clone());
            if kind == RuntimeKind::CoreCLR {
                query.schema = Some("eventpipe.frame".into());
                query.fields.push(FieldFilter {
                    name: "source_file".into(),
                    value: json!(fs::canonicalize(&workload).unwrap()),
                });
            } else {
                query.function = Some(
                    match kind {
                        RuntimeKind::Go => "main.resize",
                        RuntimeKind::HotSpot => "AssetPreview.resize",
                        _ => "resize",
                    }
                    .into(),
                );
            }
            let page = project.query(&query, QueryLimits::default(), None).unwrap();
            let evidence = page
                .records
                .iter()
                .find_map(|entry| {
                    let record = project.record(&entry.reference, None).unwrap();
                    (record
                        .function
                        .as_ref()
                        .is_some_and(|name| name.to_ascii_lowercase().contains("resize"))
                        && record
                            .location
                            .as_ref()
                            .is_some_and(|location| location.contains(fixture)))
                    .then_some((entry.reference.clone(), record))
                })
                .expect("known automatically observed fixture function/source is required");
            let measured = driver.record_ref("0");
            project.add_relationship(&run,Relationship{id:"selected-code".into(),from:measured.clone(),to:evidence.0.clone(),kind:"selected-code-for".into(),origin:Origin::Contributed,corrects:None,metadata:json!({"meaning":"caller selected code evidence for checked asset; no causal or resource ownership inferred"})}).unwrap();
            assert_eq!(
                project
                    .relationships(
                        &measured,
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
                        &evidence.0,
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
            let mut operation = Query::new(run.clone());
            operation.operation = Some("preview-operation".into());
            assert_eq!(
                project
                    .query(&operation, QueryLimits::default(), None)
                    .unwrap()
                    .records
                    .len(),
                2
            );
            assert!(matches!(
                hunin_app::begin(project.clone(), plan()).unwrap(),
                BeginCapture::Existing(_)
            ));
            project.add_context(&run,Context{id:"output-check".into(),subject:"preview-operation".into(),origin:Origin::Contributed,value:json!({"outcome":"passed","scope":"every produced byte and manifest field"}),corrects:None}).unwrap();
            retained.push(evidence);
            trials.push(Trial {
                run: run.id,
                value: report.cpu_ns.parse().unwrap(),
                conditions: BTreeMap::from([
                    ("input".into(), input_identity.clone()),
                    ("algorithm".into(), "nearest-neighbor-rgb8".into()),
                    ("configured_repeated_work".into(), count.to_string()),
                ]),
                complete_coverage: report.collection_complete,
                required_output_checks: BTreeMap::from([(
                    "every produced byte and manifest field".into(),
                    CheckOutcome::Passed,
                )]),
            });
        }
    }
    let meaning = MeasurementMeaning {
        quantity: "process CPU".into(),
        unit: "ns".into(),
        method: "user+system process counter delta, readiness through held completion snapshot"
            .into(),
        population: format!("one {kind:?} prepared process, all threads, excluding children"),
        denominator: "two selected asset previews".into(),
        aggregation: "cumulative difference".into(),
    };
    let baseline = Series {
        meaning: meaning.clone(),
        trials: trials[..2].to_vec(),
    };
    let changed = Series {
        meaning,
        trials: trials[2..].to_vec(),
    };
    let comparison = compare(&baseline, &changed).unwrap();
    assert!(
        matches!(
            comparison.status,
            ComparisonStatus::Conditional | ComparisonStatus::UndefinedRatio
        ),
        "changed repeated work and zero tick-resolution counters cannot yield an unconditional ratio"
    );
    if comparison.status == ComparisonStatus::UndefinedRatio {
        assert!(comparison.ratio.is_none());
    }
    let mut failed = changed.clone();
    let bad_path = project.directory().join("deliberately-bad.preview.ppm");
    let mut bad_bytes = expected[0].2.clone();
    bad_bytes[13] ^= 1;
    fs::write(&bad_path, &bad_bytes).unwrap();
    let failed_check = if fs::read(&bad_path).unwrap() == expected[0].2 {
        CheckOutcome::Passed
    } else {
        CheckOutcome::Failed
    };
    assert_eq!(failed_check, CheckOutcome::Failed);
    project.add_context(&project.reference("variant-1-1"),Context{id:"negative-output-check".into(),subject:"deliberately-bad-artifact".into(),origin:Origin::Contributed,value:json!({"outcome":"failed","path":bad_path,"scope":"independent negative-control artifact; original capture checks remain unchanged","expected_sha256":format!("{:x}",Sha256::digest(&expected[0].2)),"actual_sha256":format!("{:x}",Sha256::digest(&bad_bytes))}),corrects:None}).unwrap();
    failed.trials[0].required_output_checks.insert(
        "every produced byte and manifest field".into(),
        failed_check,
    );
    assert!(
        !compare(&baseline, &failed)
            .unwrap()
            .declared_output_checks_preserved
    );
    let mut incompatible = changed;
    incompatible.meaning.population = "different process set".into();
    assert_eq!(
        compare(&baseline, &incompatible).unwrap().status,
        ComparisonStatus::Incompatible
    );
    assert_eq!(
        fs::read_dir(project.directory().join("tmp"))
            .unwrap()
            .count(),
        0
    );
    drop(project);
    let reopened = Project::open(data.path(), "p").unwrap();
    for (reference, record) in retained {
        assert_eq!(reopened.record(&reference, None).unwrap(), record);
    }
    eprintln!(
        "asset-acceptance {kind:?}: two variants x two repetitions, two P6 assets, exact outputs, resources/code/context/replay/comparison/reopen passed"
    );
}

#[test]
#[ignore = "requires selected HUNIN_NODE"]
fn node_asset_investigation() {
    asset_investigation(RuntimeKind::Node, "HUNIN_NODE", "asset-preview.cjs", 10000);
}
#[test]
#[ignore = "requires selected HUNIN_DENO"]
fn deno_asset_investigation() {
    asset_investigation(RuntimeKind::Deno, "HUNIN_DENO", "asset-preview.cjs", 10000);
}
#[test]
#[ignore = "requires selected HUNIN_BUN"]
fn bun_asset_investigation() {
    asset_investigation(RuntimeKind::Bun, "HUNIN_BUN", "asset-preview.cjs", 10000);
}
#[test]
#[ignore = "requires selected HUNIN_PYTHON"]
fn cpython_asset_investigation() {
    asset_investigation(RuntimeKind::CPython, "HUNIN_PYTHON", "asset-preview.py", 16);
}
#[test]
#[ignore = "requires selected HUNIN_PYPY"]
fn pypy_asset_investigation() {
    asset_investigation(RuntimeKind::PyPy, "HUNIN_PYPY", "asset-preview.py", 16);
}
#[test]
#[ignore = "requires selected HUNIN_RUBY"]
fn ruby_asset_investigation() {
    asset_investigation(RuntimeKind::CRuby, "HUNIN_RUBY", "asset-preview.rb", 32);
}
#[test]
#[ignore = "requires selected HUNIN_JAVA"]
fn java_asset_investigation() {
    asset_investigation(
        RuntimeKind::HotSpot,
        "HUNIN_JAVA",
        "AssetPreview.java",
        50000,
    );
}
#[test]
#[ignore = "requires selected HUNIN_GO"]
fn go_asset_investigation() {
    asset_investigation(RuntimeKind::Go, "HUNIN_GO", "asset-preview.go", 50000);
}
#[test]
#[ignore = "requires selected HUNIN_PHP/HUNIN_XDEBUG"]
fn php_asset_investigation() {
    asset_investigation(RuntimeKind::PHP, "HUNIN_PHP", "asset-preview.php", 2);
}
#[test]
#[ignore = "requires selected HUNIN_DOTNET"]
fn clr_asset_investigation() {
    asset_investigation(
        RuntimeKind::CoreCLR,
        "HUNIN_DOTNET",
        "asset-preview.cs",
        5000,
    );
}

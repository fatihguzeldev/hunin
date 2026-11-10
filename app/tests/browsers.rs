use hunin_app::{BeginCapture, BrowserCapturePlan};
use hunin_core::{
    compare::{CheckOutcome, ComparisonStatus, MeasurementMeaning, Series, Trial, compare},
    control::RequestId,
    project::Project,
    query::{Context, Origin, Query, QueryLimits},
};
use hunin_runtimes::browser::BrowserKind;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::PathBuf, time::Duration};

fn asset_investigation(kind: BrowserKind, variable: &str) {
    let browser = PathBuf::from(std::env::var_os(variable).expect("set selected browser"));
    let node = PathBuf::from(std::env::var_os("HUNIN_NODE").expect("set selected Node observer"));
    let driver = match kind {
        BrowserKind::Firefox => Some(PathBuf::from(
            std::env::var_os("HUNIN_GECKODRIVER").expect("set selected geckodriver"),
        )),
        BrowserKind::Safari => Some(PathBuf::from(
            std::env::var_os("HUNIN_SAFARIDRIVER").expect("set selected safaridriver"),
        )),
        BrowserKind::Chromium => None,
    };
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("runtimes/tests/fixtures");
    let data = tempfile::tempdir().unwrap();
    let project = Project::create(data.path(), "p").unwrap();
    let investigation = project
        .create_investigation(
            "i",
            "browser asset previews",
            json!({"resource_scope":"explicit process set; no page cpu attribution"}),
        )
        .unwrap();
    let input = project.directory().join("input");
    fs::create_dir(&input).unwrap();
    let mut expected = Vec::new();
    for asset in 0..2 {
        let name = format!("asset-{asset}.ppm");
        let pixels: Vec<u8> = (0..64 * 64 * 3)
            .map(|index| ((index * 19 + asset * 71 + index / 17) & 255) as u8)
            .collect();
        let mut original = b"P6\n64 64\n255\n".to_vec();
        original.extend(&pixels);
        fs::write(input.join(&name), original).unwrap();
        let mut output = b"P6\n32 32\n255\n".to_vec();
        for y in 0..32 {
            for x in 0..32 {
                let position = ((y * 2) * 64 + x * 2) * 3;
                output.extend(&pixels[position..position + 3]);
            }
        }
        expected.push((name, format!("asset-{asset}.preview.ppm"), output));
    }
    let mut trials = Vec::new();
    let mut retained = Vec::new();
    for variant in 0..2 {
        for repetition in 0..2 {
            let passes = if kind == BrowserKind::Safari {
                100
            } else {
                10000
            } * (variant + 1);
            let run = project
                .create_run(
                    &investigation,
                    &format!("variant-{variant}-{repetition}"),
                    json!({"passes":passes,"asset_count":2,"input":"known-64x64-rgb8"}),
                )
                .unwrap();
            let output = project
                .directory()
                .join(format!("output-{variant}-{repetition}"));
            fs::create_dir(&output).unwrap();
            let request = RequestId {
                project: "p".into(),
                caller: "browser-agent".into(),
                id: run.id.clone(),
            };
            let plan = || BrowserCapturePlan {
                request: request.clone(),
                run: run.clone(),
                kind,
                browser: browser.clone(),
                node: node.clone(),
                driver: driver.clone(),
                driver_script: Some(fixtures.join("browser-assets-driver.cjs")),
                workload: fixtures.join("browser-assets.html"),
                conditions: json!({"algorithm":"nearest-neighbor-rgb8","passes":passes}),
                environment: BTreeMap::from([
                    ("PREVIEW_INPUT".into(), input.to_string_lossy().into_owned()),
                    (
                        "PREVIEW_OUTPUT".into(),
                        output.to_string_lossy().into_owned(),
                    ),
                    ("PREVIEW_REPETITIONS".into(), passes.to_string()),
                ]),
                read_paths: vec![input.clone()],
                write_paths: vec![output.clone()],
                safari_scripts: if kind == BrowserKind::Safari {
                    vec![fixtures.join("browser-assets.js")]
                } else {
                    vec![]
                },
                babel_root: (kind == BrowserKind::Safari).then(|| {
                    PathBuf::from(
                        std::env::var_os("HUNIN_BABEL_ROOT")
                            .expect("set selected Babel installation"),
                    )
                }),
                babel_node: (kind == BrowserKind::Safari).then(|| {
                    PathBuf::from(
                        std::env::var_os("HUNIN_BABEL_NODE").expect("set selected compiler Node"),
                    )
                }),
            };
            let BeginCapture::Started(mut capture) =
                hunin_app::begin_browser(project.clone(), plan()).unwrap()
            else {
                panic!("fresh browser capture")
            };
            let primary = capture.resource_source().definition().unwrap();
            assert_eq!(primary.target.as_ref().unwrap(), capture.target());
            let actual_pid = primary.metadata["instance"]["pid"].as_u64().unwrap() as u32;
            let actual_executable = primary.metadata["instance"]["executable"].as_str().unwrap();
            assert!(
                !actual_executable.ends_with("/node"),
                "primary resources must describe actual browser rather than observer helper"
            );
            let minimum_targets = if kind == BrowserKind::Safari { 1 } else { 2 };
            assert!(capture.resource_sources().count() >= minimum_targets);
            project.add_context(&run,Context{id:"caller-operation".into(),subject:"preview-operation".into(),origin:Origin::Contributed,value:json!({"assets":["asset-0.ppm","asset-1.ppm"],"resource_attribution":"none; selected process resources include other activity"}),corrects:None}).unwrap();
            capture.release().unwrap();
            let report = capture.finish(Duration::from_secs(90)).unwrap();
            assert!(report.application_success, "{}", report.application);
            assert!(!report.code_sources.is_empty());
            assert!(report.targets.len() >= minimum_targets);
            if kind == BrowserKind::Safari {
                hunin_platforms::inspect_process(actual_pid)
                    .expect("borrowed Safari application must survive observer cleanup");
            }
            assert!(
                !report.code_sources.contains(&report.collection_source),
                "collection metadata must not masquerade as page function evidence"
            );
            let manifest: Value =
                serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
            assert_eq!(manifest, report.application["output"]);
            let assets = manifest["assets"].as_array().unwrap();
            assert_eq!(assets.len(), expected.len());
            for (index, (name, destination, bytes)) in expected.iter().enumerate() {
                let actual = fs::read(output.join(destination)).unwrap();
                assert_eq!(&actual, bytes);
                assert_eq!(
                    assets[index],
                    json!({"input":name,"output":destination,"source_width":64,"source_height":64,"bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(bytes))})
                );
            }
            let mut found = None;
            for source in &report.code_sources {
                let mut query = Query::new(run.clone());
                query.source = Some(source.clone());
                query.function = Some("resizePreview".into());
                let page = project.query(&query, QueryLimits::default(), None).unwrap();
                for entry in page.records {
                    let record = project.record(&entry.reference, None).unwrap();
                    if record
                        .location
                        .as_ref()
                        .is_some_and(|location| location.contains("browser-assets.js"))
                    {
                        found = Some((entry.reference, record));
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            let evidence =
                found.expect("known page function/source must appear in exported stream evidence");
            let descriptor = project
                .source(&run, &evidence.0.source)
                .unwrap()
                .definition()
                .unwrap();
            assert_eq!(
                descriptor.metadata["parent_collection"]["source"],
                report.collection_source
            );
            assert!(!descriptor.clocks.is_empty());
            retained.push(evidence);
            assert!(matches!(
                hunin_app::begin_browser(project.clone(), plan()).unwrap(),
                BeginCapture::Existing(_)
            ));
            trials.push(Trial {
                run: run.id,
                value: report.cpu_ns.parse().unwrap(),
                conditions: BTreeMap::from([
                    ("input".into(), "known-64x64-rgb8".into()),
                    ("passes".into(), passes.to_string()),
                ]),
                complete_coverage: report.collection_complete,
                required_output_checks: BTreeMap::from([(
                    "all preview bytes and manifest".into(),
                    CheckOutcome::Passed,
                )]),
            });
            eprintln!(
                "browser-acceptance {kind:?} variant={variant} repetition={repetition} targets={} code_sources={} collection_complete={} warnings={}",
                report.targets.len(),
                report.code_sources.len(),
                report.collection_complete,
                report.warnings.len()
            );
        }
    }
    let meaning = MeasurementMeaning {
        quantity: "cpu time".into(),
        unit: "ns".into(),
        method: "sum of explicit pre-work process counter differences".into(),
        population: if kind == BrowserKind::Safari {
            "borrowed shared Safari process set; no page ownership"
        } else {
            "fresh browser's explicit process set; no page ownership"
        }
        .into(),
        denominator: "two caller-selected previews".into(),
        aggregation: "all registered process instances, user+system, children counted separately"
            .into(),
    };
    let baseline = Series {
        meaning: meaning.clone(),
        trials: trials[..2].to_vec(),
    };
    let changed = Series {
        meaning,
        trials: trials[2..].to_vec(),
    };
    assert!(matches!(
        compare(&baseline, &changed).unwrap().status,
        ComparisonStatus::Conditional | ComparisonStatus::UndefinedRatio
    ));
    assert_eq!(
        fs::read_dir(project.directory().join("tmp"))
            .unwrap()
            .count(),
        0
    );
    drop(project);
    let reopened = Project::open(data.path(), "p").unwrap();
    for (reference, expected) in retained {
        assert_eq!(reopened.record(&reference, None).unwrap(), expected);
    }
}

#[test]
#[ignore = "requires selected HUNIN_CHROME and HUNIN_NODE"]
fn chromium_asset_investigation() {
    asset_investigation(BrowserKind::Chromium, "HUNIN_CHROME");
}
#[test]
#[ignore = "requires selected HUNIN_FIREFOX/HUNIN_GECKODRIVER/HUNIN_NODE"]
fn firefox_asset_investigation() {
    asset_investigation(BrowserKind::Firefox, "HUNIN_FIREFOX");
}
#[test]
#[ignore = "requires selected Safari/safaridriver, helper Node and explicit Babel/compiler Node"]
fn safari_asset_investigation() {
    asset_investigation(BrowserKind::Safari, "HUNIN_SAFARI");
}

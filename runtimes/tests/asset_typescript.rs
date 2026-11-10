use hunin_core::{
    execution::PreparedProcess,
    project::Project,
    query::{Query, QueryLimits},
    records::SourceDefinition,
};
use hunin_runtimes::{
    Limits, RuntimeKind, prepare,
    source_maps::{Binding, BoundMap, digest},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires selected HUNIN_TSC/HUNIN_NODE/HUNIN_DENO/HUNIN_BUN"]
fn asset_typescript_locations_remain_queryable_after_all_generated_inputs_are_removed() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "ts-assets").unwrap();
    let investigation = project
        .create_investigation("i", "typescript asset previews", json!({}))
        .unwrap();
    let run = project
        .create_run(&investigation, "r", json!({"repetitions":10000}))
        .unwrap();
    let tmp = project.directory().join("tmp");
    let build = tmp.join("build");
    let input = tmp.join("input");
    fs::create_dir(&build).unwrap();
    fs::create_dir(&input).unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/asset-preview.cts");
    assert!(
        fs::read_to_string(&source)
            .unwrap()
            .lines()
            .nth(28)
            .unwrap()
            .starts_with("function resize("),
        "independent original declaration oracle"
    );
    let mut compile = Command::new(std::env::var("HUNIN_TSC").unwrap());
    compile
        .args([
            "--target",
            "es2022",
            "--module",
            "commonjs",
            "--sourceMap",
            "--inlineSources",
            "--outDir",
        ])
        .arg(&build)
        .arg(&source);
    hunin_platforms::tools::require_success(
        hunin_platforms::tools::run(&mut compile, &build, Duration::from_secs(30), 64 * 1024)
            .unwrap(),
    )
    .unwrap();
    let generated = build.join("asset-preview.cjs");
    let script = fs::read(&generated).unwrap();
    let bytes = fs::read(build.join("asset-preview.cjs.map")).unwrap();
    let binding = Binding {
        generated_sha256: digest(&script),
        map_sha256: digest(&bytes),
    };
    let bound = BoundMap::parse(&script, &bytes, &binding, Limits::default()).unwrap();
    let maps = bound.records("typescript-build").unwrap();
    let map_source = project
        .register_source(
            &run,
            SourceDefinition {
                id: "compiler-map".into(),
                target: None,
                schemas: maps.schemas,
                clocks: vec![],
                interpretation: "source-map-v3".into(),
                metadata: maps.metadata,
            },
        )
        .unwrap();
    map_source.ready(json!({})).unwrap();
    for batch in maps.records.chunks(256) {
        map_source.append(batch).unwrap();
    }
    map_source.close(json!({"complete":true})).unwrap();
    let mut image = b"P6\n63 45\n255\n".to_vec();
    image.extend((0..63 * 45 * 3).map(|i| (i % 251) as u8));
    fs::write(input.join("image.ppm"), image).unwrap();
    let mut checks = vec![];
    for (kind, key) in [
        (RuntimeKind::Node, "HUNIN_NODE"),
        (RuntimeKind::Deno, "HUNIN_DENO"),
        (RuntimeKind::Bun, "HUNIN_BUN"),
    ] {
        let area = tmp.join(key);
        let output = tmp.join(format!("{key}-output"));
        fs::create_dir(&area).unwrap();
        fs::create_dir(&output).unwrap();
        let exe = PathBuf::from(std::env::var(key).unwrap());
        let mut prepared = prepare(kind, &exe, &generated, &area).unwrap();
        prepared
            .launch
            .environment
            .insert("HUNIN_FINALIZE_GATE".into(), "1".into());
        prepared
            .configure(
                &BTreeMap::from([
                    ("PREVIEW_INPUT".into(), input.to_string_lossy().into_owned()),
                    (
                        "PREVIEW_OUTPUT".into(),
                        output.to_string_lossy().into_owned(),
                    ),
                    ("PREVIEW_REPETITIONS".into(), "10000".into()),
                    ("HUNIN_SCRIPT_SOURCES".into(), "1".into()),
                ]),
                std::slice::from_ref(&input),
                std::slice::from_ref(&output),
            )
            .unwrap();
        let mut process = PreparedProcess::spawn(&prepared.launch).unwrap();
        let target = project
            .register_target(&run, key, json!({"pid":process.id()}), true)
            .unwrap();
        let native = project
            .register_source(&run, prepared.definition(&format!("{key}-code"), &target))
            .unwrap();
        process.await_ready(Duration::from_secs(10)).unwrap();
        native.ready(json!({})).unwrap();
        process.release().unwrap();
        let until = Instant::now() + Duration::from_secs(30);
        while !process.work_complete().unwrap_or_else(|e| {
            panic!("{kind:?}: {e:?} {:?}", process.wait(Duration::from_secs(1)))
        }) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(2));
        }
        let extraction = prepared.extract(key, Limits::default()).unwrap();
        let report = &extraction
            .records
            .iter()
            .find(|r| r.schema == "prepared.report")
            .unwrap()
            .payload;
        assert_eq!(report["outcome"], "completed", "{kind:?}: {report}");
        let scripts = report["scripts"].as_array().unwrap();
        let actual = scripts
            .iter()
            .find(|s| kind == RuntimeKind::Bun || s["sha256"] == binding.generated_sha256)
            .unwrap();
        let projected = if kind == RuntimeKind::Bun {
            bound.project_bun(report, &extraction.records, key).unwrap()
        } else {
            bound
                .project_v8(
                    actual["scriptId"].as_str().unwrap(),
                    actual["sha256"].as_str().unwrap(),
                    &extraction.records,
                    key,
                )
                .unwrap()
        };
        let resize = projected
            .records
            .iter()
            .find(|r| r.function.as_deref() == Some("resize"))
            .expect("mapped resize evidence");
        assert_eq!(resize.payload["line"], 28);
        assert!(
            resize.payload["source"]
                .as_str()
                .unwrap()
                .ends_with("asset-preview.cts")
        );
        let mapped_id = format!("{key}-original");
        checks.push((mapped_id.clone(), resize.location.clone().unwrap()));
        let mapped = project
            .register_source(
                &run,
                SourceDefinition {
                    id: mapped_id,
                    target: Some(target),
                    schemas: projected.schemas,
                    clocks: vec![],
                    interpretation: "build-bound-source-map-v3".into(),
                    metadata: projected.metadata,
                },
            )
            .unwrap();
        mapped.ready(json!({"basis":"actual script bytes; version-specific Bun coordinate binding where applicable"})).unwrap();
        for batch in projected.records.chunks(256) {
            mapped.append(batch).unwrap();
        }
        mapped.close(json!({"complete":true})).unwrap();
        for batch in extraction.records.chunks(256) {
            native.append(batch).unwrap();
        }
        native.close(json!({"complete":true})).unwrap();
        process.finish().unwrap();
        assert!(process.wait(Duration::from_secs(10)).unwrap().success);
        let mut expected = b"P6\n32 32\n255\n".to_vec();
        for y in 0..32 {
            for x in 0..32 {
                for c in 0..3 {
                    expected.push(((((y * 45 / 32) * 63 + x * 63 / 32) * 3 + c) % 251) as u8);
                }
            }
        }
        assert_eq!(
            fs::read(output.join("image.preview.ppm")).unwrap(),
            expected
        );
        let manifest: Value =
            serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest, report["output"]);
    }
    fs::remove_dir_all(&tmp).unwrap();
    drop(map_source);
    drop(project);
    let reopened = Project::open(directory.path(), "ts-assets").unwrap();
    for (source, location) in checks {
        let mut query = Query::new(run.clone());
        query.source = Some(source);
        query.function = Some("resize".into());
        query.location = Some(location);
        let page = reopened
            .query(&query, QueryLimits::default(), None)
            .unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].record.as_ref().unwrap().payload["line"], 28);
    }
}

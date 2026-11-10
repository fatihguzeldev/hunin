use hunin_core::execution::PreparedProcess;
use hunin_runtimes::{
    Limits, RuntimeKind, prepare,
    source_maps::{Binding, BoundMap, digest},
};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires HUNIN_TSC and selected HUNIN_NODE/HUNIN_DENO/HUNIN_BUN absolute executables"]
fn real_typescript_maps_only_the_executed_generated_build_on_all_three_runtimes() {
    let compiler = PathBuf::from(std::env::var("HUNIN_TSC").expect("selected tsc"));
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/preview.cts");
    let build = tempfile::tempdir().unwrap();
    let status = Command::new(compiler)
        .args([
            "--target",
            "es2022",
            "--module",
            "commonjs",
            "--sourceMap",
            "--inlineSources",
            "--outDir",
        ])
        .arg(build.path())
        .arg(&source)
        .status()
        .unwrap();
    assert!(status.success());
    let generated = build.path().join("preview.cjs");
    let script = fs::read(&generated).unwrap();
    let map = fs::read(build.path().join("preview.cjs.map")).unwrap();
    let binding = Binding {
        generated_sha256: digest(&script),
        map_sha256: digest(&map),
    };
    let bound = BoundMap::parse(&script, &map, &binding, Limits::default()).unwrap();
    for (kind, var) in [
        (RuntimeKind::Node, "HUNIN_NODE"),
        (RuntimeKind::Deno, "HUNIN_DENO"),
        (RuntimeKind::Bun, "HUNIN_BUN"),
    ] {
        eprintln!("checking {kind:?} executed-source binding and final gate");
        let area = tempfile::tempdir().unwrap();
        let exe = PathBuf::from(std::env::var(var).expect("selected runtime"));
        let mut prepared = prepare(kind, &exe, &generated, area.path()).unwrap();
        prepared
            .launch
            .environment
            .insert("HUNIN_SCRIPT_SOURCES".into(), "1".into());
        prepared
            .launch
            .environment
            .insert("HUNIN_FINALIZE_GATE".into(), "1".into());
        let mut process = PreparedProcess::spawn(&prepared.launch).unwrap();
        process.await_ready(Duration::from_secs(10)).unwrap();
        process.release().unwrap();
        let until = Instant::now() + Duration::from_secs(30);
        while !process.work_complete().unwrap_or_else(|e| {
            panic!(
                "{kind:?}: {e:?}; {:?}",
                process.wait(Duration::from_secs(2))
            )
        }) {
            assert!(Instant::now() < until, "work deadline");
            std::thread::sleep(Duration::from_millis(2));
        }
        let extracted = prepared.extract("process", Limits::default()).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(&prepared.report_path).unwrap()).unwrap();
        assert_eq!(report["outcome"], "completed", "{kind:?}");
        let actual = report["scripts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["sha256"] == binding.generated_sha256 || kind == RuntimeKind::Bun)
            .expect("actual executed script identity");
        let script_id = actual["scriptId"].as_str().unwrap();
        let executed_sha = actual["sha256"].as_str().unwrap();
        let projected = if kind == RuntimeKind::Bun {
            let mut corrupt = report.clone();
            corrupt["scripts"][0]["source"] = serde_json::json!("another executed build");
            assert!(
                bound
                    .project_bun(&corrupt, &extracted.records, "process")
                    .is_err()
            );
            corrupt = report.clone();
            corrupt["runtime"]["bun"] = serde_json::json!("1.4.5");
            assert!(
                bound
                    .project_bun(&corrupt, &extracted.records, "process")
                    .is_err()
            );
            bound
                .project_bun(&report, &extracted.records, "process")
                .unwrap()
        } else {
            bound
                .project_v8(script_id, executed_sha, &extracted.records, "process")
                .unwrap()
        };
        assert!(
            projected.records.iter().any(|r| {
                r.function.as_deref() == Some("resize")
                    && r.location
                        .as_ref()
                        .is_some_and(|s| s.contains("preview.cts:1:"))
            }),
            "{kind:?} projected={:?}; resize nodes={:?}",
            projected.records,
            extracted
                .records
                .iter()
                .filter(|r| r.function.as_deref() == Some("resize"))
                .collect::<Vec<_>>()
        );
        assert!(
            bound
                .project_v8(script_id, &"0".repeat(64), &extracted.records, "process")
                .is_err()
        );
        let mut mapped = 0;
        for node in extracted
            .records
            .iter()
            .filter(|r| r.schema == "v8.node" && r.payload["callFrame"]["functionName"] == "resize")
        {
            let frame = &node.payload["callFrame"];
            assert_eq!(frame["scriptId"], script_id);
            let line = frame["lineNumber"].as_u64().unwrap() as u32;
            let column = frame["columnNumber"].as_u64().unwrap() as u32;
            let original = bound
                .lookup(line, column)
                .expect("source mapped declaration");
            assert!(original.source.ends_with("preview.cts"));
            assert_eq!(original.line, 0);
            mapped += 1;
        }
        assert!(mapped > 0, "{kind:?} must expose generated resize identity");
        for (i, pixel) in report["output"]["pixels"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            assert_eq!(
                pixel.as_u64().unwrap(),
                ((((i & 255) * 3 + 7) ^ (i & 255)) & 255) as u64
            );
        }
        if let Err(error) = process.finish() {
            panic!(
                "{kind:?} final gate failed: {error:?}; {:?}",
                process.wait(Duration::from_secs(2))
            );
        }
        assert!(process.wait(Duration::from_secs(10)).unwrap().success);
    }
    let mut wrong = script.clone();
    wrong.extend(b"\n// different build");
    assert!(BoundMap::parse(&wrong, &map, &binding, Limits::default()).is_err());
    let retained = bound.records("build").unwrap();
    let reopened = BoundMap::from_records(&retained.records).unwrap();
    assert_eq!(bound.lookup(3, 0), reopened.lookup(3, 0));
}

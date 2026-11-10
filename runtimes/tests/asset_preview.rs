use hunin_core::execution::PreparedProcess;
use hunin_runtimes::{Limits, RuntimeKind, prepare};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires eight selected runtime/toolchain executables"]
fn real_asset_directories_produce_independently_checked_ppm_files_and_manifest() {
    let data = tempfile::tempdir().unwrap();
    let input = data.path().join("input");
    fs::create_dir(&input).unwrap();
    let dimensions = [("wide.ppm", 65, 47), ("tall.ppm", 49, 73)];
    for (name, width, height) in dimensions {
        let mut bytes = format!("P6\n# independent fixture\n{width} {height}\n255\n").into_bytes();
        for y in 0..height {
            for x in 0..width {
                for channel in 0..3 {
                    bytes.push(((x * 19 + y * 7 + channel * 61) % 256) as u8);
                }
            }
        }
        fs::write(input.join(name), bytes).unwrap();
    }
    for (kind, key, fixture) in [
        (RuntimeKind::Node, "HUNIN_NODE", "asset-preview.cjs"),
        (RuntimeKind::Deno, "HUNIN_DENO", "asset-preview.cjs"),
        (RuntimeKind::Bun, "HUNIN_BUN", "asset-preview.cjs"),
        (RuntimeKind::CPython, "HUNIN_PYTHON", "asset-preview.py"),
        (RuntimeKind::PyPy, "HUNIN_PYPY", "asset-preview.py"),
        (RuntimeKind::CRuby, "HUNIN_RUBY", "asset-preview.rb"),
        (RuntimeKind::Go, "HUNIN_GO", "asset-preview.go"),
        (RuntimeKind::HotSpot, "HUNIN_JAVA", "AssetPreview.java"),
    ] {
        let area = tempfile::tempdir().unwrap();
        let output = data.path().join(key);
        fs::create_dir(&output).unwrap();
        let exe = PathBuf::from(std::env::var(key).expect("selected absolute executable"));
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        let mut prepared = prepare(kind, &exe, &fixture, area.path()).unwrap();
        prepared
            .launch
            .environment
            .insert("HUNIN_FINALIZE_GATE".into(), "1".into());
        let environment = BTreeMap::from([
            (
                "PREVIEW_REPETITIONS".into(),
                match kind {
                    RuntimeKind::CPython | RuntimeKind::PyPy => "16",
                    RuntimeKind::CRuby => "32",
                    RuntimeKind::Go | RuntimeKind::HotSpot => "50000",
                    _ => "10000",
                }
                .into(),
            ),
            ("PREVIEW_INPUT".into(), input.to_string_lossy().into_owned()),
            (
                "PREVIEW_OUTPUT".into(),
                output.to_string_lossy().into_owned(),
            ),
        ]);
        prepared
            .configure(
                &environment,
                std::slice::from_ref(&input),
                std::slice::from_ref(&output),
            )
            .unwrap();
        let mut process = PreparedProcess::spawn(&prepared.launch).unwrap();
        process.await_ready(Duration::from_secs(10)).unwrap();
        process.release().unwrap();
        let until = Instant::now() + Duration::from_secs(30);
        while !process.work_complete().unwrap_or_else(|e| {
            panic!("{kind:?} {e:?}: {:?}", process.wait(Duration::from_secs(1)))
        }) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(2));
        }
        let extraction = prepared
            .extract("asset-preview", Limits::default())
            .unwrap();
        let report = extraction
            .records
            .iter()
            .find(|r| r.schema == "prepared.report")
            .unwrap();
        assert_eq!(
            report.payload["outcome"], "completed",
            "{kind:?}: {}",
            report.payload
        );
        assert!(
            extraction.records.iter().any(|r| r
                .function
                .as_ref()
                .is_some_and(|name| name == "resize" || name.ends_with(".resize"))),
            "{kind:?} known resize evidence"
        );
        process.finish().unwrap();
        assert!(process.wait(Duration::from_secs(10)).unwrap().success);
        let manifest: Value =
            serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["algorithm"], "nearest-neighbor-rgb8");
        assert_eq!(manifest["width"], 32);
        assert_eq!(manifest["height"], 32);
        assert_eq!(manifest["assets"].as_array().unwrap().len(), 2);
        for (name, width, height) in dimensions {
            let filename = name.replace(".ppm", ".preview.ppm");
            let bytes = fs::read(output.join(&filename)).unwrap();
            let mut expected = b"P6\n32 32\n255\n".to_vec();
            for y in 0..32 {
                for x in 0..32 {
                    for channel in 0..3 {
                        expected.push(
                            (((x * width / 32) * 19 + (y * height / 32) * 7 + channel * 61) % 256)
                                as u8,
                        );
                    }
                }
            }
            assert_eq!(bytes, expected, "{kind:?} {name}");
            let entry = manifest["assets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["input"] == name)
                .unwrap();
            assert_eq!(
                entry,
                &json!({"input":name,"output":filename,"source_width":width,"source_height":height,"bytes":expected.len(),"sha256":format!("{:x}",Sha256::digest(&expected))})
            );
        }
    }
}

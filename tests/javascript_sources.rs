use hunin::{
    collection::{CaptureRequest, prepare},
    importers::{self, ImportedProfile},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn sha(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}
fn capture(
    runtime: &str,
    executable: &str,
    worker: bool,
    indexed: bool,
) -> (tempfile::TempDir, Vec<PathBuf>) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir(root.join("build")).unwrap();
    fs::create_dir(root.join("capture")).unwrap();
    fs::write(
        root.join("work.ts"),
        include_str!("../src/importers/fixtures/javascript-source-map/work.ts"),
    )
    .unwrap();
    let mut code =
        include_str!("../src/importers/fixtures/javascript-source-map/build/work.js").to_owned();
    if worker {
        code = code.replace(
            "console.log(tsHotspot());",
            "import {parentPort} from 'node:worker_threads'; parentPort.postMessage(tsHotspot());",
        );
    }
    fs::write(root.join("build/work.js"), code).unwrap();
    let map: Value = serde_json::from_str(include_str!(
        "../src/importers/fixtures/javascript-source-map/build/work.js.map"
    ))
    .unwrap();
    let map = if indexed {
        json!({"version":3,"file":"work.js","sections":[{"offset":{"line":0,"column":0},"map":map}]})
    } else {
        map
    };
    fs::write(
        root.join("build/work.js.map"),
        serde_json::to_vec(&map).unwrap(),
    )
    .unwrap();
    let mut command = vec![executable.to_owned()];
    if runtime == "deno" {
        command.extend(["run".into(), format!("--allow-read={}", root.display())]);
    }
    if worker {
        fs::write(root.join("main.mjs"),"import {Worker}from'node:worker_threads';const worker=new Worker(new URL('./build/work.js',import.meta.url));worker.on('message',value=>console.log(value));").unwrap();
        command.push(root.join("main.mjs").display().to_string());
    } else {
        command.push(
            root.join(if runtime == "node" {
                "build/work.js"
            } else {
                "work.ts"
            })
            .display()
            .to_string(),
        );
    }
    let plan = prepare(&CaptureRequest {
        command,
        cwd: root.into(),
        artifact_dir: root.join("capture"),
        run_id: "script-control".into(),
        runtime: runtime.into(),
        capture_mode: Default::default(),
        code_options: None,
        env: BTreeMap::new(),
    })
    .unwrap();
    let output = Command::new(&plan.command[0])
        .args(&plan.command[1..])
        .envs(&plan.env)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let paths = fs::read_dir(root.join("capture"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|s| s == "cpuprofile"))
        .collect::<Vec<_>>();
    assert!(!paths.is_empty());
    (directory, paths)
}
fn hot(profile: &ImportedProfile) -> Vec<&Value> {
    profile
        .records
        .iter()
        .flat_map(|r| r["stack"].as_array().into_iter().flatten())
        .filter(|f| f["function"] == "tsHotspot")
        .collect()
}
fn import_hot(paths: &[PathBuf]) -> (PathBuf, ImportedProfile) {
    paths
        .iter()
        .find_map(|path| {
            let profile = importers::import(path, "v8-cpuprofile", "script-control").unwrap();
            if hot(&profile).is_empty() {
                None
            } else {
                Some((path.clone(), profile))
            }
        })
        .expect("no actual hotspot sample")
}
fn script_sidecar(root: &Path) -> PathBuf {
    fs::read_dir(root.join("capture"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("scripts-")
                && fs::read_to_string(p).unwrap().contains("work.js\"")
        })
        .unwrap()
}
#[test]
fn actual_node_map_uses_observed_script_identity_and_survives_source_deletion() {
    let node = std::env::var("HUNIN_TEST_NODE").unwrap_or_else(|_| "node".into());
    for indexed in [false, true] {
        let (directory, paths) = capture("node", &node, false, indexed);
        let (path, profile) = import_hot(&paths);
        let hot = hot(&profile);
        assert!(
            hot.iter()
                .all(|f| f["line"] == 42
                    && f["file"].as_str().is_some_and(|s| s.ends_with("work.ts"))),
            "{:?}",
            hot[0]
        );
        let frame = hot[0];
        let code_id = frame["code_id"].as_str().unwrap();
        assert!(code_id.starts_with("sha256:"));
        let snapshot = profile
            .source_snapshots
            .iter()
            .find(|s| {
                s.uri == frame["file"].as_str().unwrap() && s.provenance["code_id"] == code_id
            })
            .unwrap();
        assert_eq!(
            snapshot.content,
            include_str!("../src/importers/fixtures/javascript-source-map/work.ts")
        );
        assert_eq!(
            snapshot.provenance["source_position_basis"],
            "runtime_linked_source_map"
        );
        assert_eq!(snapshot.provenance["compiler_mapping_proof"], false);
        hunin::ingestion::validate("script-control", &profile.records, &Default::default())
            .unwrap();
        fs::remove_file(directory.path().join("work.ts")).unwrap();
        fs::remove_dir_all(directory.path().join("build")).unwrap();
        let replay = importers::import(&path, "v8-cpuprofile", "script-control").unwrap();
        assert_eq!(replay.records, profile.records);
        assert_eq!(replay.input_fingerprint, profile.input_fingerprint);
    }
}
#[test]
fn actual_node_worker_map_preserves_its_isolate_namespace() {
    let node = std::env::var("HUNIN_TEST_NODE").unwrap_or_else(|_| "node".into());
    let (_directory, paths) = capture("node", &node, true, false);
    let (_, profile) = import_hot(&paths);
    assert!(hot(&profile).iter().all(|f| f["line"] == 42));
    assert!(
        profile
            .records
            .iter()
            .filter(|r| r["kind"] == "sample")
            .all(|r| r["tid"] != "0")
    );
}
#[test]
fn wrong_or_stale_maps_cannot_redirect_to_current_typescript_lines() {
    let node = std::env::var("HUNIN_TEST_NODE").unwrap_or_else(|_| "node".into());
    let (directory, paths) = capture("node", &node, false, false);
    let (path, _) = import_hot(&paths);
    let sidecar = script_sidecar(directory.path());
    let original = fs::read_to_string(&sidecar).unwrap();
    for failure in 0..5 {
        let mut rows: Vec<Value> = original
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        for row in &mut rows {
            if row["url"].as_str().is_some_and(|s| s.ends_with("work.js")) {
                if failure == 1 {
                    row["generated_sha256"] = json!("wrong");
                } else if failure == 3 {
                    row["pid"] = json!(row["pid"].as_u64().unwrap() + 1);
                } else if failure == 4 {
                    continue;
                } else {
                    let mut map: Value =
                        serde_json::from_str(row["source_map"]["content"].as_str().unwrap())
                            .unwrap();
                    if failure == 0 {
                        map["file"] = json!("wrong.js");
                    } else {
                        map["sourcesContent"] = json!([include_str!(
                            "../src/importers/fixtures/javascript-source-map/work.ts"
                        )
                        .replace("tsHotspot", "wrongName")]);
                    }
                    let text = serde_json::to_string(&map).unwrap();
                    row["source_map"]["sha256"] = json!(sha(&text));
                    row["source_map"]["content"] = json!(text);
                }
            }
        }
        if failure == 4 {
            let mut changed = rows
                .iter()
                .find(|r| r["url"].as_str().is_some_and(|s| s.ends_with("work.js")))
                .unwrap()
                .clone();
            let source = changed["generated_code"].as_str().unwrap().to_owned()
                + "\n// different live script version\n";
            changed["generated_sha256"] = json!(sha(&source));
            changed["generated_code"] = json!(source);
            rows.push(changed);
        }
        fs::write(
            &sidecar,
            rows.iter()
                .map(|r| serde_json::to_string(r).unwrap() + "\n")
                .collect::<String>(),
        )
        .unwrap();
        let profile = importers::import(&path, "v8-cpuprofile", "script-control").unwrap();
        assert!(
            hot(&profile).iter().all(|f| f["file"]
                .as_str()
                .is_some_and(|s| s.ends_with("work.js"))
                && f["line"] == 1)
        );
        assert!(!profile.diagnostics.is_empty());
        assert!(
            failure == 4
                || !profile
                    .source_snapshots
                    .iter()
                    .any(|s| s.uri.ends_with("work.ts")),
            "invalid map must not publish an original-source selection"
        );
    }
}
#[test]
#[ignore = "Requires explicit actual Bun and Deno runtimes; absent cells are not support evidence"]
fn bun_and_deno_keep_runtime_reported_typescript_positions() {
    for (runtime, variable) in [("bun", "HUNIN_TEST_BUN"), ("deno", "HUNIN_TEST_DENO")] {
        let executable = std::env::var(variable).unwrap_or_else(|_| panic!("{variable} required"));
        let (_directory, paths) = capture(runtime, &executable, false, false);
        let (_, profile) = import_hot(&paths);
        assert!(hot(&profile).iter().all(|f| f["line"] == 42));
        assert!(profile.records.iter().any(|r| {
            r["attributes"]["script_sources"]
                .as_array()
                .is_some_and(|a| {
                    a.iter()
                        .any(|p| p["source_position_basis"] == "runtime_reported")
                })
        }));
        assert!(!profile.source_snapshots.is_empty());
        let frame = hot(&profile)[0];
        let original = profile
            .source_snapshots
            .iter()
            .find(|s| {
                s.uri == frame["file"].as_str().unwrap()
                    && s.provenance["code_id"] == frame["code_id"]
            })
            .expect("runtime reported TS position must retain separately scoped original bytes");
        assert_eq!(
            original.content,
            include_str!("../src/importers/fixtures/javascript-source-map/work.ts")
        );
    }
}

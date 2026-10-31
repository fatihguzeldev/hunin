use hunin::{
    execution::{Engine, StartRequest},
    model::runs::{Artifact, Run},
    operations::Operations,
    storage::Store,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, time::Duration};

fn fixture() -> PathBuf {
    let root = std::env::var_os("HUNIN_ASSETS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    let fixture = root.join("collectors/javascript/dist/code/fixtures/workload.mjs");
    assert!(
        fixture.is_file(),
        "build the authored TypeScript collector fixtures first: {}",
        fixture.display()
    );
    fixture
}

fn request(cwd: &std::path::Path, duration_ms: u64, maximum_bytes: u64) -> StartRequest {
    serde_json::from_value(json!({
        "name":"independent native-code custody", "cwd":cwd, "runtime":"node",
        "command":[std::env::var("HUNIN_TEST_NODE").unwrap_or_else(|_| "node".into()),fixture(),duration_ms.to_string()],
        "capture_mode":"code-origin", "code_options":{"compiler_export":false,"sampling_interval_us":1000,"max_artifact_bytes":maximum_bytes},
        "expected_exit_code":0,"timeout_seconds":10
    })).unwrap()
}

async fn finish(engine: &Engine, id: &str) -> Run {
    match tokio::time::timeout(Duration::from_secs(60), engine.wait(id)).await {
        Ok(result) => result.unwrap(),
        Err(_) => {
            engine.cancel(id).await.unwrap();
            let _ = tokio::time::timeout(Duration::from_secs(15), engine.wait(id)).await;
            panic!("owned code-origin recording did not settle within its test bound");
        }
    }
}

fn bytes(store: &Store, run: &Run, artifact: &Artifact) -> Vec<u8> {
    fs::read(store.artifact_path(&run.id, &artifact.id).unwrap()).unwrap()
}

#[tokio::test]
async fn opt_in_code_origin_preserves_checksums_and_publishes_inspectable_relative_samples() {
    let project = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    let engine = Engine::new(store.clone());
    let started = engine
        .start(request(project.path(), 600, 64 * 1024 * 1024))
        .await
        .unwrap();
    let run = finish(&engine, &started.id).await;
    assert_eq!(run.exit_code, Some(0), "{:?}", run.diagnostics);
    assert_eq!(run.correctness["passed"], true, "{:?}", run.diagnostics);
    assert_eq!(run.status, "completed", "{:?}", run.diagnostics);
    assert_eq!(
        run.source["collection_completion"]["state"], "complete",
        "{:?}",
        run.diagnostics
    );
    let artifacts = store.artifacts(&run.id).unwrap();
    let stdout = artifacts.iter().find(|a| a.format == "stdout").unwrap();
    let output: Value = serde_json::from_slice(&bytes(&store, &run, stdout)).unwrap();
    assert_eq!(output["fixture"], "independent-wasm-emitter-v1");
    assert!(output["runs"].as_u64().unwrap() > 0);
    let sums =
        [3u64, 7].map(|multiplier| ((200000u64 * 200001 / 2 + 200000) * multiplier) as u32 as i32);
    assert_eq!(output["checksums"], json!(sums));
    let manifest = artifacts
        .iter()
        .find(|a| a.name.ends_with(".v8code.json"))
        .unwrap();
    let manifest: Value = serde_json::from_slice(&bytes(&store, &run, manifest)).unwrap();
    assert_eq!(manifest["status"], "target_exit_observed");
    assert_eq!(
        manifest["wasm_custody"],
        "post_workload_live_module_snapshot"
    );
    assert!(manifest["compiler_exports"].as_array().unwrap().is_empty());
    assert!(manifest["diagnostics"].as_array().unwrap().is_empty());
    assert!(!manifest["logs"].as_array().unwrap().is_empty());
    for log in manifest["logs"].as_array().unwrap() {
        for key in ["path", "ll_path"] {
            let artifact = artifacts
                .iter()
                .find(|a| Some(a.name.as_str()) == log[key].as_str())
                .unwrap();
            assert!(!bytes(&store, &run, artifact).is_empty());
        }
    }
    let modules = manifest["wasm"].as_array().unwrap();
    assert_eq!(modules.len(), 2);
    assert_ne!(modules[0]["sha256"], modules[1]["sha256"]);
    for module in modules {
        let artifact = artifacts
            .iter()
            .find(|a| Some(a.name.as_str()) == module["path"].as_str())
            .unwrap();
        let content = bytes(&store, &run, artifact);
        assert!(content.starts_with(b"\0asm\x01\0\0\0"));
        assert_eq!(
            format!("{:x}", Sha256::digest(content)),
            module["sha256"].as_str().unwrap()
        );
    }
    let records = store.records(&run.id).unwrap();
    assert!(
        records
            .iter()
            .any(|row| row["attributes"]["code_instance"]["representation"] == "wasm")
    );
    let (sample, frame_index) = records
        .iter()
        .filter(|row| row["kind"] == "sample")
        .find_map(|sample| {
            sample["stack"]
                .as_array()
                .unwrap()
                .iter()
                .position(|frame| frame["code_instance_id"].is_string())
                .map(|index| (sample, index))
        })
        .expect("native ticks must publish at least one frame with real retained code identity");
    assert_eq!(sample["attributes"]["clock_alignment"], "runtime_relative");
    let domain = sample["attributes"]["clock"].as_str().unwrap();
    let operations = Operations::new(store.clone());
    let epoch = operations
        .call(
            "profiles.inspect",
            json!({"run_id":run.id,"weight_unit":"samples"}),
        )
        .await
        .unwrap();
    assert_eq!(epoch["sample_count"], 0);
    assert!(
        epoch["clock_domains"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == domain)
    );
    let relative = operations
        .call(
            "profiles.inspect",
            json!({"run_id":run.id,"weight_unit":"samples","clock_domain":domain}),
        )
        .await
        .unwrap();
    assert_eq!(relative["clock_domain"], domain);
    assert!(relative["sample_count"].as_u64().unwrap() > 0);
    assert!(!relative["bounds"].is_null());
    let inspected = operations.call("code.inspect",json!({"run_id":run.id,"observation_id":sample["id"],"frame_index":frame_index,"clock_domain":domain})).await.unwrap();
    assert_eq!(
        inspected["anchor"]["code_instance_id"],
        sample["stack"][frame_index]["code_instance_id"]
    );
    assert_eq!(inspected["clock_domain"], domain);
    assert_eq!(inspected["anchor"]["aligned"], true);
    let epoch_code = operations
        .call(
            "code.inspect",
            json!({"run_id":run.id,"observation_id":sample["id"],"frame_index":frame_index}),
        )
        .await
        .unwrap();
    assert!(epoch_code["clock_domain"].is_null());
    assert_eq!(epoch_code["anchor"]["aligned"], false);
    assert!(
        inspected["coverage"]["total"]["sample_count"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(
        inspected["input_artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|proof| proof["status"] == "metadata_matches")
    );
    // Native sampling and custody do not establish compiler maps or guest cost ownership.
    assert_eq!(
        run.source["collector_settings"]["code_options"]["compiler_export"],
        false
    );
}

#[tokio::test]
async fn tiny_capture_budget_stops_owned_target_and_keeps_raw_evidence() {
    let project = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    let engine = Engine::new(store.clone());
    let started = engine
        .start(request(project.path(), 30000, 4096))
        .await
        .unwrap();
    let run = finish(&engine, &started.id).await;
    assert_eq!(run.status, "failed", "{:?}", run.diagnostics);
    assert_eq!(run.source["capture_budget"]["status"], "stopped");
    assert!(
        run.source["capture_budget"]["maximum_observed_bytes"]
            .as_u64()
            .unwrap()
            > 4096
    );
    assert!(
        run.source["capture_budget"]["failure"]
            .as_str()
            .unwrap()
            .contains("budget exceeded")
    );
    assert_eq!(run.source["collection_completion"]["state"], "incomplete");
    assert_eq!(run.source["collection_status"], "failed");
    assert!(
        !run.diagnostics
            .iter()
            .any(|s| s.contains("recording timeout"))
    );
    assert_ne!(run.source["target_outcome"]["status"], "canceled");
    let capture = data.path().join("captures").join(&run.id);
    assert!(capture.is_dir());
    let originals = fs::read_dir(&capture)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .collect::<Vec<_>>();
    assert!(
        originals
            .iter()
            .any(|p| fs::metadata(p).unwrap().len() > 4096),
        "raw producer files must remain untruncated"
    );
    let artifacts = store.artifacts(&run.id).unwrap();
    assert!(artifacts.iter().any(|a| a.size > 4096));
    for artifact in artifacts {
        let retained = bytes(&store, &run, &artifact);
        assert_eq!(retained.len() as u64, artifact.size);
        assert_eq!(format!("{:x}", Sha256::digest(retained)), artifact.sha256);
    }
}

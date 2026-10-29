use super::*;
use std::{collections::BTreeMap, fs};

#[test]
fn incomplete_raw_retention_preserves_target_correctness_and_rejects_all_normalized_prefixes() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::storage::Store::open(root.path()).unwrap();
    let engine = Engine::new(store.clone());
    let directory = root.path().join("captures/raw-incomplete");
    fs::create_dir_all(directory.join("sdk")).unwrap();
    fs::write(directory.join("stdout.txt"), b"target-ok\n").unwrap();
    fs::write(directory.join("stderr.txt"), b"").unwrap();
    fs::write(directory.join("a-complete.bin"), b"early").unwrap();
    fs::write(directory.join("z-oversize.bin"), vec![8; 1000]).unwrap();
    let record = json!({"schema_version":1,"kind":"event","id":"would-accept","run_id":"raw-incomplete","producer_id":"fixture","pid":1,"tid":"main","timestamp_us":1,"name":"valid-prefix","basis":"observed"});
    fs::write(directory.join("sdk/events.jsonl"), format!("{record}\n")).unwrap();
    fs::write(directory.join("cpu.cpuprofile"), serde_json::to_vec(&json!({"nodes":[{"id":1,"callFrame":{"functionName":"hotspot"}}],"samples":[1],"timeDeltas":[1],"startTime":1,"endTime":2})).unwrap()).unwrap();
    let run: Run = serde_json::from_value(json!({"id":"raw-incomplete","name":"capture","status":"running","command":["node"],"cwd":directory,"runtime":"node","started_at_us":1,"finished_at_us":null,"exit_code":null,"diagnostics":[],"source":{"resource_observation":{},"correctness_recipe":{"expected_exit_code":0,"expected_stdout_sha256":format!("{:x}",sha2::Sha256::digest(b"target-ok\n"))}},"correctness":{}})).unwrap();
    store.save_run(&run).unwrap();
    let request = CaptureRequest {
        command: vec!["node".into()],
        cwd: directory.clone(),
        artifact_dir: directory.clone(),
        run_id: run.id.clone(),
        runtime: "node".into(),
        env: BTreeMap::new(),
    };
    let plan = serde_json::from_value(
        json!({"command":["node"],"output":{"kind":"profiles","format":"v8-cpuprofile"}}),
    )
    .unwrap();
    engine
        .finish_with_raw_limits(
            run,
            Target {
                exit_code: Some(0),
                outcome: None,
                canceled: false,
                timed_out: false,
            },
            Capture {
                request,
                plan,
                start_failure: None,
            },
            resources::Retention::default(),
            retention::Limits {
                file_bytes: 512,
                total_source_bytes: 4096,
                ..Default::default()
            },
        )
        .unwrap();
    let finished = store.run("raw-incomplete").unwrap();
    assert_eq!(finished.status, "failed");
    assert_eq!(finished.exit_code, Some(0));
    assert_eq!(finished.correctness["passed"], true);
    assert_eq!(finished.source["target_outcome"]["status"], "completed");
    assert_eq!(finished.source["raw_retention"]["complete"], false);
    assert_eq!(finished.source["raw_retention"]["inventory_complete"], true);
    assert_eq!(
        finished.source["raw_retention"]["failure"]["reason"],
        "source_bytes"
    );
    assert_eq!(finished.source["sdk_ingestion"]["status"], "skipped");
    assert_eq!(finished.source["sdk_ingestion"]["completion"], "unknown");
    assert_eq!(finished.source["collection_status"], "failed");
    assert!(store.records("raw-incomplete").unwrap().is_empty());
    assert!(
        store
            .artifacts("raw-incomplete")
            .unwrap()
            .iter()
            .any(|artifact| artifact.name == "a-complete.bin")
    );
    assert!(
        store
            .artifacts("raw-incomplete")
            .unwrap()
            .iter()
            .any(|artifact| artifact.name == "cpu.cpuprofile")
    );
    assert_eq!(
        fs::read(directory.join("stdout.txt")).unwrap(),
        b"target-ok\n"
    );
    assert_eq!(
        fs::metadata(directory.join("z-oversize.bin"))
            .unwrap()
            .len(),
        1000
    );
    assert!(directory.join("sdk/events.jsonl").is_file());
}

use sha2::Digest;

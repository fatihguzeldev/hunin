use hunin::{importers, model::runs::Run, operations::Operations, storage::Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::PathBuf};

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cpu.json");
    fs::write(
        &path,
        include_bytes!("../src/importers/fixtures/native-lifetimes/cpu.json"),
    )
    .unwrap();
    fs::write(
        path.with_extension("syms.json"),
        include_bytes!("../src/importers/fixtures/native-lifetimes/cpu.syms.json"),
    )
    .unwrap();
    let mut sidecar = Vec::new();
    flate2::read::GzDecoder::new(
        include_bytes!("../src/importers/fixtures/native-lifetimes/cpu.lifetimes.json.gz")
            .as_slice(),
    )
    .read_to_end(&mut sidecar)
    .unwrap();
    fs::write(path.with_extension("lifetimes.json"), sidecar).unwrap();
    (directory, path)
}

#[tokio::test]
async fn actual_same_address_a_b_a_maps_samples_to_distinct_instances_and_store() {
    let (_directory, path) = fixture();
    let profile = importers::import(&path, "samply", "native-lifetime").unwrap();
    let phases: Vec<_> = profile
        .records
        .iter()
        .filter(|r| {
            r["name"] == "module.observed_lifetime"
                && r["attributes"]["module"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("phase_"))
        })
        .collect();
    assert_eq!(phases.len(), 3);
    assert_eq!(
        phases
            .iter()
            .map(|r| &r["attributes"]["start_avma"])
            .collect::<std::collections::HashSet<_>>()
            .len(),
        1
    );
    assert_eq!(
        phases
            .iter()
            .map(|r| &r["id"])
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    for phase in &phases {
        assert_eq!(phase["kind"], "span");
        assert_eq!(
            phase["attributes"]["boundary_basis"]["start"],
            "dyld_snapshot"
        );
        assert_eq!(
            phase["attributes"]["boundary_basis"]["end"],
            "dyld_snapshot"
        );
        assert_eq!(phase["attributes"]["exact_event_timestamps"], false);
        assert_eq!(phase["attributes"]["evidence_complete"], true);
        assert!(phase["attributes"]["end_window_ms"][0].is_number());
        assert!(
            profile
                .records
                .iter()
                .any(|r| r["relation"] == "sampled_in_code_instance" && r["to"] == phase["id"])
        );
    }
    assert_ne!(
        phases[0]["attributes"]["library"]["codeId"],
        phases[1]["attributes"]["library"]["codeId"]
    );
    let first_end =
        phases[0]["timestamp_us"].as_u64().unwrap() + phases[0]["duration_us"].as_u64().unwrap();
    let second_start = phases[1]["timestamp_us"].as_u64().unwrap();
    assert!(
        second_start - first_end > 40_000,
        "actual fixture preserves the deliberately unloaded gap"
    );
    let midpoint = first_end + (second_start - first_end) / 2;
    assert!(phases.iter().all(|r| {
        let start = r["timestamp_us"].as_u64().unwrap();
        let end = start + r["duration_us"].as_u64().unwrap();
        midpoint < start || midpoint >= end
    }));
    let store_root = tempfile::tempdir().unwrap();
    let store = Store::open(store_root.path()).unwrap();
    store
        .save_run(&Run {
            id: "native-lifetime".into(),
            name: "A/B/A".into(),
            status: "completed".into(),
            command: vec!["fixture".into()],
            cwd: ".".into(),
            runtime: "native".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: Some(0),
            diagnostics: vec![],
            source: json!({}),
            correctness: json!({}),
        })
        .unwrap();
    let imported = Operations::new(store.clone())
        .call(
            "recordings.import",
            json!({"run_id":"native-lifetime","path":path,"format":"samply"}),
        )
        .await
        .unwrap();
    assert_eq!(imported["accepted"], true);
    let stored = store.records("native-lifetime").unwrap();
    assert_eq!(stored.len(), profile.records.len());
    let by_id = stored
        .iter()
        .map(|record| (record["id"].as_str().unwrap(), record))
        .collect::<std::collections::HashMap<_, _>>();
    for record in &profile.records {
        let id = record["id"].as_str().unwrap();
        assert_eq!(
            by_id.get(id).copied(),
            Some(record),
            "published native observation {id}"
        );
    }
    let artifacts = store.artifacts("native-lifetime").unwrap();
    assert!(
        artifacts
            .iter()
            .any(|artifact| artifact.sha256 == profile.raw_sha256)
    );
    for image in &profile.binary_snapshots {
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.format == "native-image" && artifact.sha256 == image.sha256)
            .expect("native code proof must be retained before observation publication");
        let bytes = fs::read(
            store
                .artifact_path("native-lifetime", &artifact.id)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(bytes, image.content);
    }
    assert_eq!(
        profile.records,
        importers::import(&path, "samply", "native-lifetime")
            .unwrap()
            .records
    );
}

#[test]
fn unrelated_or_tampered_sample_mapping_cannot_be_accepted() {
    let (_directory, path) = fixture();
    let sidecar = path.with_extension("lifetimes.json");
    let original: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    let mut unrelated = original.clone();
    unrelated["raw_profile_sha256"] = json!("0".repeat(64));
    fs::write(&sidecar, serde_json::to_vec(&unrelated).unwrap()).unwrap();
    assert!(importers::import(&path, "samply", "run").is_err());
    let mut bad_address = original.clone();
    let sample = bad_address["events"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| {
            e["kind"] == "sample"
                && e["frames"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|f| f["load_id"].is_string())
        })
        .unwrap();
    let frame = sample["frames"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["load_id"].is_string())
        .unwrap();
    frame["lookup_address"] = json!("0x1");
    fs::write(&sidecar, serde_json::to_vec(&bad_address).unwrap()).unwrap();
    assert!(importers::import(&path, "samply", "run").is_err());
    let mut lost = original;
    lost["truncated"] = json!(true);
    fs::write(&sidecar, serde_json::to_vec(&lost).unwrap()).unwrap();
    let partial = importers::import(&path, "samply", "run").unwrap();
    assert!(partial.diagnostics.iter().any(|d| d.contains("incomplete")));
    assert!(
        partial
            .records
            .iter()
            .filter(|r| r["name"] == "module.observed_lifetime")
            .all(|r| r["attributes"]["evidence_complete"] == false)
    );
    assert_eq!(
        partial.sidecar_sha256[&sidecar],
        format!("{:x}", Sha256::digest(fs::read(&sidecar).unwrap()))
    );
}

#[test]
fn reported_perf_loss_marks_lifetimes_and_links_incomplete_without_truncation() {
    let (_directory, path) = fixture();
    let sidecar = path.with_extension("lifetimes.json");
    let mut input: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    assert_eq!(input["truncated"], false);
    input["events"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"loss","basis":"perf_lost_events","count":181}));
    let raw = serde_json::to_vec(&input).unwrap();
    fs::write(&sidecar, &raw).unwrap();

    let profile = importers::import(&path, "samply", "reported-loss").unwrap();
    assert!(profile.diagnostics.iter().any(|d| d.contains("incomplete")));
    for records in [
        profile
            .records
            .iter()
            .filter(|r| r["name"] == "module.observed_lifetime")
            .collect::<Vec<_>>(),
        profile
            .records
            .iter()
            .filter(|r| r["relation"] == "sampled_in_code_instance")
            .collect::<Vec<_>>(),
    ] {
        assert!(
            !records.is_empty(),
            "loss does not erase the observed evidence"
        );
        assert!(
            records
                .iter()
                .all(|r| r["attributes"]["evidence_complete"] == false)
        );
    }
    assert_eq!(fs::read(&sidecar).unwrap(), raw);
    assert_eq!(
        profile.sidecar_sha256[&sidecar],
        format!("{:x}", Sha256::digest(&raw))
    );
}

#[test]
fn root_exit_two_survives_normalization_and_unknown_outcome_stays_unknown() {
    let (_directory, path) = fixture();
    let sidecar = path.with_extension("lifetimes.json");
    let mut input: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    let pid = input["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "mapping")
        .unwrap()["process"]["pid"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    input["target_outcome"] = json!({"state":"exited","pid":pid,"exit_code":2,"signal":null,"launch_observed_at_us":1792777622000000u64,"wait_observed_at_us":1792777625000000u64,"source":"root_target_os_wait_status"});
    fs::write(&sidecar, serde_json::to_vec(&input).unwrap()).unwrap();
    let profile = importers::import(&path, "samply", "outcome").unwrap();
    let event = profile
        .records
        .iter()
        .find(|r| r["name"] == "process.wait_outcome")
        .unwrap();
    assert_eq!(event["pid"], pid);
    assert_eq!(event["timestamp_us"], 1792777625000000u64);
    assert_eq!(event["attributes"]["target_outcome"]["exit_code"], 2);
    assert_eq!(event["basis"], "observed");
    hunin::ingestion::validate(
        "outcome",
        &profile.records,
        &std::collections::HashSet::new(),
    )
    .unwrap();
    input["target_outcome"]["signal"] = json!(9);
    fs::write(&sidecar, serde_json::to_vec(&input).unwrap()).unwrap();
    assert!(importers::import(&path, "samply", "outcome").is_err());
    input["target_outcome"] = json!({"state":"unknown","source":"root_target_os_wait_status"});
    fs::write(&sidecar, serde_json::to_vec(&input).unwrap()).unwrap();
    let unknown = importers::import(&path, "samply", "outcome").unwrap();
    assert!(
        unknown
            .records
            .iter()
            .all(|r| r["name"] != "process.wait_outcome")
    );
}

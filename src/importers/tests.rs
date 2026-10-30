use super::*;
use serde_json::json;
use std::path::Path;
use std::{collections::HashSet, fs};

#[test]
fn real_native_recording_resolves_known_hotspot_and_validates() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/importers/fixtures/native.json");
    let profile = import(&path, "firefox", "control").unwrap();
    assert!(profile.records.len() > 100);
    assert!(
        profile
            .records
            .iter()
            .flat_map(|r| r["stack"].as_array().into_iter().flatten())
            .any(|f| f["function"] == "hunin_cpu_hotspot"
                && f["file"]
                    .as_str()
                    .is_some_and(|s| s.ends_with("native_cpu.c")))
    );
    crate::ingestion::validate("control", &profile.records, &HashSet::new()).unwrap();
    let replay = import(&path, "firefox", "control").unwrap();
    assert_eq!(
        profile.records, replay.records,
        "reimport must keep stable observation identity"
    );
}

#[test]
fn real_graal_recording_retains_epoch_sample_times() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/importers/fixtures/graal.json");
    let profile = import(&path, "graal-cpusampler", "control").unwrap();
    assert!(
        profile
            .records
            .iter()
            .all(|r| r["timestamp_us"].as_u64().unwrap() > 1_700_000_000_000_000)
    );
    assert!(
        profile
            .records
            .iter()
            .flat_map(|r| r["stack"].as_array().unwrap())
            .any(|f| f["function"] == "hunin_python_hotspot")
    );
    crate::ingestion::validate("control", &profile.records, &HashSet::new()).unwrap();
}

#[test]
fn real_go_pprof_preserves_cpu_time_and_source_hotspot() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/importers/fixtures/go.pprof");
    let profile = import(&path, "pprof", "control").unwrap();
    assert!(
        profile
            .records
            .iter()
            .all(|r| r["weight_unit"] == "microseconds"
                && r["attributes"]["temporal_resolution"] == "aggregate")
    );
    assert!(
        profile
            .records
            .iter()
            .flat_map(|r| r["stack"].as_array().unwrap())
            .any(|f| f["function"]
                .as_str()
                .is_some_and(|n| n.ends_with("huninGoHotspot")))
    );
    crate::ingestion::validate("control", &profile.records, &HashSet::new()).unwrap();
}

#[test]
fn symbol_sidecar_changes_input_identity_and_resolves_reanalysis() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/importers/fixtures");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native.json");
    fs::copy(root.join("native.json"), &path).unwrap();
    let before = import(&path, "firefox", "control").unwrap();
    fs::copy(
        root.join("native.syms.json"),
        path.with_extension("syms.json"),
    )
    .unwrap();
    let after = import(&path, "firefox", "control").unwrap();
    assert_ne!(before.input_fingerprint, after.input_fingerprint);
    assert_ne!(before.records[0]["id"], after.records[0]["id"]);
    assert!(
        after
            .records
            .iter()
            .flat_map(|r| r["stack"].as_array().into_iter().flatten())
            .any(|f| f["function"] == "hunin_cpu_hotspot")
    );
    assert_eq!(after.sidecars.len(), 1);
}

#[test]
fn deno_worker_clock_anchor_does_not_merge_worker_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join("CPU.1792774105123.42.worker-1.cpuprofile");
    fs::write(&path,serde_json::to_vec(&json!({"startTime":10,"nodes":[{"id":1,"callFrame":{"functionName":"workerHot"}}],"samples":[1],"timeDeltas":[3]})).unwrap()).unwrap();
    fs::write(directory.path().join("clock-42-0.meta.json"),r#"{"pid":42,"tid":"0","profile_time_origin_us":1700000000000000,"clock_source":"control"}"#).unwrap();
    let profile = import(&path, "v8-cpuprofile", "control").unwrap();
    assert_eq!(profile.records[0]["tid"], "worker-1");
    assert_eq!(profile.records[0]["pid"], 42);
    assert_eq!(profile.records[0]["timestamp_us"], 1700000000000013u64);
    crate::ingestion::validate("control", &profile.records, &HashSet::new()).unwrap();
}

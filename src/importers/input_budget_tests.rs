use super::*;
use std::fs;

#[test]
fn valid_raw_profile_above_private_byte_limit_is_rejected_before_decode() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("raw.jsonl");
    let bytes = b"{\"run_id\":\"run\"}\n";
    fs::write(&path, bytes).unwrap();
    let limits = inputs::Limits {
        raw_bytes: bytes.len() as u64,
        ..Default::default()
    };
    assert_eq!(
        import_with_limits(&path, "jsonl", "run", None, limits, None)
            .unwrap()
            .records
            .len(),
        1
    );
    assert_eq!(
        import(&path, "jsonl", "run").unwrap().input_fingerprint,
        "5e15e0dc7499ec197d52e8d1aa95134b490e9519982eed5d6f1136d0740c2d1b"
    );
    let error = import_with_limits(
        &path,
        "jsonl",
        "run",
        None,
        inputs::Limits {
            raw_bytes: limits.raw_bytes - 1,
            ..limits
        },
        None,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("profile-normalization-input-v1"));
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn decoder_uses_exact_captured_raw_and_sidecar_bytes_after_originals_change() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("CPU.123456789012.44.0.cpuprofile");
    let raw = serde_json::to_vec(&json!({"startTime":10,"endTime":13,"nodes":[{"id":1,"callFrame":{"functionName":"originalHotspot"}}],"samples":[1],"timeDeltas":[3]})).unwrap();
    let metadata = serde_json::to_vec(
        &json!({"pid":99,"profile_time_origin_us":1000,"clock_source":"captured"}),
    )
    .unwrap();
    fs::write(&path, &raw).unwrap();
    let sidecar = path.with_extension("meta.json");
    fs::write(&sidecar, &metadata).unwrap();
    let baseline = import(&path, "v8-cpuprofile", "run").unwrap();
    let (snapshot, digest) =
        capture_inputs(&path, "v8-cpuprofile", inputs::Limits::default(), None).unwrap();
    fs::write(&path, b"changed original raw; no longer valid JSON").unwrap();
    fs::write(&sidecar, br#"{"pid":888,"profile_time_origin_us":2000}"#).unwrap();
    let profile = decode_snapshot(
        &path,
        "v8-cpuprofile",
        "run",
        None,
        &snapshot,
        digest,
        &mut OutputBudget::default(),
    )
    .unwrap();
    assert_eq!(profile.records, baseline.records);
    assert_eq!(profile.input_fingerprint, baseline.input_fingerprint);
    assert_eq!(profile.raw_sha256, baseline.raw_sha256);
    assert_eq!(profile.sidecar_sha256, baseline.sidecar_sha256);
    assert_eq!(profile.records[0]["pid"], 99);
    assert_eq!(profile.records[0]["timestamp_us"], 1013);
    assert_eq!(
        profile.records[0]["stack"][0]["function"],
        "originalHotspot"
    );
    assert_eq!(profile.source, path);
    assert_eq!(profile.sidecars, vec![sidecar]);
}

#[test]
fn explicit_normalization_deadline_refuses_work_before_snapshot_creation() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("raw.jsonl");
    fs::write(&path, b"{\"run_id\":\"run\"}\n").unwrap();
    let error =
        import_controlled(&path, "jsonl", "run", Some(std::time::Instant::now())).unwrap_err();
    assert!(error.to_string().contains("deadline"));
    assert!(path.is_file());
}

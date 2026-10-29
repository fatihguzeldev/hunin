use super::*;

#[test]
fn raw_and_aggregate_sidecar_limits_bound_actual_snapshot_capture() {
    let root = tempfile::tempdir().unwrap();
    let raw = root.path().join("profile.bin");
    fs::write(&raw, b"1234").unwrap();
    let limits = Limits {
        raw_bytes: 4,
        sidecar_bytes: 8,
        decompressed_bytes: 4,
    };
    let mut snapshot = Snapshot::new(limits, None).unwrap();
    let mut digest = Sha256::new();
    snapshot.capture_raw(&raw, &mut digest).unwrap();
    assert_eq!(snapshot.decoded().unwrap(), b"1234");
    for name in ["one", "two", "three"] {
        fs::write(root.path().join(name), b"side").unwrap();
    }
    snapshot
        .capture_sidecar(&root.path().join("one"), 4, &mut digest)
        .unwrap();
    snapshot
        .capture_sidecar(&root.path().join("two"), 4, &mut digest)
        .unwrap();
    assert_eq!(snapshot.sidecar_reads, 8);
    assert!(
        snapshot
            .capture_sidecar(&root.path().join("three"), 4, &mut digest)
            .is_err()
    );
    assert_eq!(snapshot.sidecar_reads, 8);
    let mut tiny = Snapshot::new(
        Limits {
            raw_bytes: 3,
            ..limits
        },
        None,
    )
    .unwrap();
    assert!(tiny.capture_raw(&raw, &mut Sha256::new()).is_err());
    assert_eq!(fs::read(&raw).unwrap(), b"1234");
}

#[test]
fn gzip_budget_rejects_oversized_decoded_output_without_returning_a_prefix() {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(b"12345").unwrap();
    let bytes = encoder.finish().unwrap();
    let root = tempfile::tempdir().unwrap();
    let raw = root.path().join("profile.gz");
    fs::write(&raw, bytes).unwrap();
    let mut snapshot = Snapshot::new(
        Limits {
            raw_bytes: 100,
            sidecar_bytes: 100,
            decompressed_bytes: 5,
        },
        None,
    )
    .unwrap();
    snapshot.capture_raw(&raw, &mut Sha256::new()).unwrap();
    assert_eq!(snapshot.decoded().unwrap(), b"12345");
    snapshot.limits.decompressed_bytes = 4;
    assert!(
        snapshot
            .decoded()
            .unwrap_err()
            .to_string()
            .contains("decoded bytes")
    );
}

#[test]
fn original_mutation_cannot_change_the_decoder_snapshot_and_private_changes_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let raw = root.path().join("profile.bin");
    fs::write(&raw, b"original").unwrap();
    let mut snapshot = Snapshot::new(Limits::default(), None).unwrap();
    snapshot.capture_raw(&raw, &mut Sha256::new()).unwrap();
    let directory = snapshot.directory.clone();
    fs::write(&raw, b"mutated after capture").unwrap();
    assert_eq!(snapshot.decoded().unwrap(), b"original");
    snapshot.verify().unwrap();
    assert_eq!(
        snapshot.raw_sha256(),
        format!("{:x}", Sha256::digest(b"original"))
    );
    fs::write(snapshot.path(), b"modified").unwrap();
    assert!(
        snapshot
            .verify()
            .unwrap_err()
            .to_string()
            .contains("snapshot changed")
    );
    drop(snapshot);
    assert!(!directory.exists());
    assert_eq!(fs::read(&raw).unwrap(), b"mutated after capture");
}

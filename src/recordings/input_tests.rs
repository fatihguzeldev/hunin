use super::*;

fn fixture() -> (tempfile::TempDir, Store, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("store")).unwrap();
    store.save_run(&serde_json::from_value(json!({"id":"bounded-import","name":"raw","status":"completed","command":[],"cwd":".","runtime":"imported","started_at_us":1,"finished_at_us":2,"exit_code":0,"diagnostics":[],"source":{},"correctness":{}})).unwrap()).unwrap();
    let path = root.path().join("raw.jsonl");
    fs::write(&path, format!("{}\n", json!({"schema_version":1,"kind":"event","id":"event","run_id":"bounded-import","producer_id":"fixture","pid":1,"tid":"main","timestamp_us":1,"basis":"observed","name":"bounded"}))).unwrap();
    (root, store, path)
}

#[test]
fn publication_rehash_and_expired_deadline_never_accept_a_valid_prefix() {
    let (_root, store, path) = fixture();
    for deadline in [None, Some(std::time::Instant::now())] {
        let profile = crate::importers::import(&path, "jsonl", "bounded-import").unwrap();
        let mut limits = crate::importers::NormalizationInputLimits::default();
        if deadline.is_none() {
            limits.raw_bytes = fs::metadata(&path).unwrap().len() - 1;
        }
        assert!(publish_with_limits(&store, "bounded-import", profile, limits, deadline).is_err());
        assert!(store.records("bounded-import").unwrap().is_empty());
        assert_eq!(store.documents("profile-inputs").unwrap().len(), 0);
    }
    assert!(path.is_file());
}

#[test]
fn reanalysis_restore_byte_limit_preserves_the_original_reference_and_result() {
    let (_root, store, path) = fixture();
    let profile = crate::importers::import(&path, "jsonl", "bounded-import").unwrap();
    publish(&store, "bounded-import", profile).unwrap();
    let before = store.records_snapshot("bounded-import").unwrap();
    let raw = store
        .artifacts("bounded-import")
        .unwrap()
        .into_iter()
        .find(|artifact| artifact.format == "jsonl")
        .unwrap();
    let limits = crate::importers::NormalizationInputLimits {
        raw_bytes: raw.size - 1,
        ..Default::default()
    };
    assert!(reanalyze_with_limits(&store, "bounded-import", &raw.id, limits, None).is_err());
    let after = store.records_snapshot("bounded-import").unwrap();
    assert_eq!(before.result_identity, after.result_identity);
    assert_eq!(before.records, after.records);
    assert!(
        store
            .artifact_path("bounded-import", &raw.id)
            .unwrap()
            .is_file()
    );
}

#[test]
fn restore_bounds_manifest_file_work_and_refuses_conflicting_names() {
    let (_root, store, path) = fixture();
    publish(
        &store,
        "bounded-import",
        crate::importers::import(&path, "jsonl", "bounded-import").unwrap(),
    )
    .unwrap();
    let raw = store
        .artifacts("bounded-import")
        .unwrap()
        .into_iter()
        .find(|artifact| artifact.format == "jsonl")
        .unwrap();
    let key = format!("bounded-import:{}", raw.sha256);
    let original = store.document("profile-inputs", &key).unwrap();
    let before = store.records_snapshot("bounded-import").unwrap();
    let mut duplicate = original.clone();
    duplicate["sidecars"] = json!(vec![serde_json::to_value(&raw).unwrap(); 1024]);
    store
        .save_document("profile-inputs", &key, &duplicate)
        .unwrap();
    assert!(
        !reanalyze(&store, "bounded-import", &raw.id)
            .unwrap()
            .accepted
    );
    duplicate["sidecars"] = json!(vec![serde_json::to_value(&raw).unwrap(); 1025]);
    store
        .save_document("profile-inputs", &key, &duplicate)
        .unwrap();
    let error = reanalyze(&store, "bounded-import", &raw.id).err().unwrap();
    let error = error.downcast_ref::<ReanalysisSidecarLimit>().unwrap();
    assert_eq!(error.maximum, 1024);
    assert_eq!(error.observed, 1025);
    let different = store
        .add_artifact_bytes(
            "bounded-import",
            &raw.name,
            "profile-sidecar",
            b"other retained bytes",
        )
        .unwrap();
    let mut conflicting = original;
    conflicting["sidecars"] = json!([different]);
    store
        .save_document("profile-inputs", &key, &conflicting)
        .unwrap();
    assert!(
        reanalyze(&store, "bounded-import", &raw.id)
            .err()
            .unwrap()
            .to_string()
            .contains("filenames disagree")
    );
    let after = store.records_snapshot("bounded-import").unwrap();
    assert_eq!(before.result_identity, after.result_identity);
    assert_eq!(before.records, after.records);
    assert!(
        store
            .artifact_path("bounded-import", &raw.id)
            .unwrap()
            .is_file()
    );
}

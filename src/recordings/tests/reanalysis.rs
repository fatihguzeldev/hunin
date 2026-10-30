use super::*;
use crate::recordings::{
    publication::publish_controlled,
    reanalysis::{ReanalysisSidecarLimit, reanalyze_controlled, reanalyze_with_limits},
};

#[test]
fn reanalysis_restore_byte_limit_preserves_the_original_reference_and_result() {
    let (_root, store, path) = fixture();
    let profile = crate::importers::import(&path, "jsonl", "bounded-import").unwrap();
    publish_controlled(&store, "bounded-import", profile, None).unwrap();
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
    publish_controlled(
        &store,
        "bounded-import",
        crate::importers::import(&path, "jsonl", "bounded-import").unwrap(),
        None,
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
        !reanalyze_controlled(&store, "bounded-import", &raw.id, None)
            .unwrap()
            .accepted
    );
    duplicate["sidecars"] = json!(vec![serde_json::to_value(&raw).unwrap(); 1025]);
    store
        .save_document("profile-inputs", &key, &duplicate)
        .unwrap();
    let error = reanalyze_controlled(&store, "bounded-import", &raw.id, None)
        .err()
        .unwrap();
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
        reanalyze_controlled(&store, "bounded-import", &raw.id, None)
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

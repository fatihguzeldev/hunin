use hunin::{model::runs::Run, operations::Operations, storage::Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;

#[tokio::test]
async fn managed_native_reanalysis_survives_removing_original_recording_sidecars_and_images() {
    let original = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/importers/fixtures/native-reload");
    for name in ["phase_a", "phase_b"] {
        fs::copy(
            fixtures.join(format!("{name}.macho")),
            original.path().join(format!("{name}.dylib")),
        )
        .unwrap();
    }
    let mut raw: Value =
        serde_json::from_slice(&fs::read(fixtures.join("cpu.json")).unwrap()).unwrap();
    for library in raw["libs"].as_array_mut().unwrap() {
        if library["name"]
            .as_str()
            .is_some_and(|s| s.starts_with("phase_"))
        {
            library["path"] = json!(original.path().join(library["name"].as_str().unwrap()));
        }
    }
    let path = original.path().join("cpu.json");
    fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    fs::copy(
        fixtures.join("cpu.syms.json"),
        original.path().join("cpu.syms.json"),
    )
    .unwrap();
    let store = Store::open(data.path()).unwrap();
    let operations = Operations::new(store.clone());
    let imported = operations
        .call("recordings.import", json!({"path":path,"format":"firefox"}))
        .await
        .unwrap();
    let id = imported["run"]["id"].as_str().unwrap();
    let records = store.records(id).unwrap();
    assert!(
        records
            .iter()
            .flat_map(|r| r["stack"].as_array().into_iter().flatten())
            .any(|f| f["module"] == "phase_b.dylib"
                && f["function"] == "[unresolved: native image identity mismatch]")
    );
    let artifact = store
        .artifacts(id)
        .unwrap()
        .into_iter()
        .find(|a| a.format == "firefox")
        .unwrap();
    assert!(
        store
            .artifacts(id)
            .unwrap()
            .iter()
            .filter(|a| a.format == "native-image")
            .count()
            >= 2
    );
    original.close().unwrap();
    let reopened = Operations::new(Store::open(data.path()).unwrap());
    let result = reopened
        .call(
            "recordings.reanalyze",
            json!({"run_id":id,"artifact_id":artifact.id}),
        )
        .await
        .unwrap();
    assert_eq!(result["accepted"], false);
    assert_eq!(store.records(id).unwrap(), records);
    assert_eq!(result["summary"], imported["summary"]);
}

#[tokio::test]
async fn failed_import_retains_original_bytes_without_publishing_completed_observations() {
    let data = tempfile::tempdir().unwrap();
    let input = data.path().join("invalid.cpuprofile");
    fs::write(&input, b"not a JSON profile").unwrap();
    let store = Store::open(&data.path().join("store")).unwrap();
    assert!(
        Operations::new(store.clone())
            .call(
                "recordings.import",
                json!({"path":input,"format":"v8-cpuprofile"})
            )
            .await
            .is_err()
    );
    let run = store.runs(10, 0).unwrap().pop().unwrap();
    assert_eq!(run.status, "failed");
    assert!(store.records(&run.id).unwrap().is_empty());
    let artifact = store.artifacts(&run.id).unwrap().pop().unwrap();
    assert_eq!(
        fs::read(store.artifact_path(&run.id, &artifact.id).unwrap()).unwrap(),
        b"not a JSON profile"
    );
}

#[tokio::test]
async fn virtual_source_versions_require_the_measured_code_identity() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store
        .save_run(&Run {
            id: "source-control".into(),
            name: "source".into(),
            status: "completed".into(),
            command: vec![],
            cwd: "/removed/project".into(),
            runtime: "node".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: Some(0),
            diagnostics: vec![],
            source: json!({}),
            correctness: json!({}),
        })
        .unwrap();
    for (version, content) in [("a", "original source"), ("b", "different source")] {
        let artifact = store
            .add_artifact_bytes(
                "source-control",
                &format!("{version}.ts"),
                "resolved-source",
                content.as_bytes(),
            )
            .unwrap();
        let digest = format!("{:x}", Sha256::digest(content.as_bytes()));
        let document = json!({"run_id":"source-control","source_snapshots":[{"uri":"webpack:///source.ts","artifact_id":artifact.id,"sha256":digest,"provenance":{"code_id":version}}]});
        store
            .publish(
                "source-control",
                version,
                version,
                &[],
                Some(("profile-inputs", version, &document)),
            )
            .unwrap();
    }
    let operations = Operations::new(store.clone());
    assert!(
        operations
            .call(
                "source.read",
                json!({"run_id":"source-control","path":"webpack:///source.ts"})
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("multiple measured source versions")
    );
    let original = operations
        .call(
            "source.read",
            json!({"run_id":"source-control","path":"webpack:///source.ts","code_id":"a"}),
        )
        .await
        .unwrap();
    assert_eq!(original["content"], "original source");
    assert_eq!(original["source_kind"], "recorded_source_map");
    assert_eq!(original["build_mapping_verified"], false);

    // Repeated imports share immutable source bytes; distinct versions consume
    // one recording-wide budget even across independent imported profiles.
    for version in 0..15u8 {
        store
            .add_artifact_bytes(
                "source-control",
                &format!("large-{version}.ts"),
                "resolved-source",
                &vec![version; 2 * 1024 * 1024],
            )
            .unwrap();
    }
    store
        .add_artifact_bytes(
            "source-control",
            "same-bytes-other-name.ts",
            "resolved-source",
            &vec![0; 2 * 1024 * 1024],
        )
        .unwrap();
    assert!(
        store
            .add_artifact_bytes(
                "source-control",
                "over-budget.ts",
                "resolved-source",
                &vec![16; 2 * 1024 * 1024]
            )
            .unwrap_err()
            .to_string()
            .contains("32 MiB recording limit")
    );
}

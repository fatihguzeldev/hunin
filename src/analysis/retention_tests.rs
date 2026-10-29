use super::*;
use crate::model::runs::Run;

fn fixture() -> (tempfile::TempDir, Store, PathBuf, Value) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("store")).unwrap();
    store
        .save_run(&Run {
            id: "recording".into(),
            name: "retention control".into(),
            status: "completed".into(),
            command: vec![],
            cwd: "/fixture".into(),
            runtime: "fixture".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: Some(0),
            diagnostics: vec![],
            source: json!({}),
            correctness: json!({}),
        })
        .unwrap();
    let directory = root.path().join("attempt");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("input.json"), b"input").unwrap();
    let document = json!({"id":"attempt","run_id":"recording","status":"failed","diagnostics":["original failure"],"execution_dir":directory});
    (root, store, directory, document)
}
fn assert_no_pending(directory: &Path) {
    assert!(!fs::read_dir(directory).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".pending")
    }));
}

#[test]
fn retained_bytes_match_the_original_pretty_document_and_ignore_workspace_descendants() {
    let (_root, store, directory, mut document) = fixture();
    let expected = serde_json::to_vec_pretty(&document).unwrap();
    fs::create_dir(directory.join("workspace")).unwrap();
    fs::File::create(directory.join("workspace/large-unselected-file"))
        .unwrap()
        .set_len(Limits::default().bytes + 1)
        .unwrap();
    retain(&store, &mut document, &directory, Limits::default(), || {
        Ok(())
    })
    .unwrap();
    assert_eq!(fs::read(directory.join("attempt.json")).unwrap(), expected);
    assert_eq!(document["raw_retention"]["complete"], true);
    assert_eq!(document["raw_retention"]["selected_files"], 2);
    assert_eq!(
        document["raw_retention"]["actual_read_bytes"],
        expected.len() as u64 + 5
    );
    assert_eq!(
        document["raw_retention"]["attempt_metadata_bytes"],
        expected.len()
    );
    assert_eq!(store.artifacts("recording").unwrap().len(), 2);
    assert!(directory.join("workspace/large-unselected-file").is_file());
    assert_no_pending(&directory);
}

#[test]
fn oversized_named_evidence_reports_incomplete_and_preserves_original_bytes() {
    let (_root, store, directory, mut document) = fixture();
    let output = directory.join("stdout.jsonl");
    fs::File::create(&output).unwrap().set_len(4097).unwrap();
    let error = retain(
        &store,
        &mut document,
        &directory,
        Limits {
            bytes: 4096,
            ..Default::default()
        },
        || Ok(()),
    )
    .unwrap_err();
    assert!(
        error
            .downcast_ref::<crate::storage::ArtifactByteLimit>()
            .is_some()
    );
    assert_eq!(document["raw_retention"]["complete"], false);
    assert_eq!(
        document["raw_retention"]["original_workspace_preserved"],
        true
    );
    assert!(
        document["raw_retention"]["actual_read_bytes"]
            .as_u64()
            .unwrap()
            <= 4096
    );
    assert_eq!(fs::metadata(output).unwrap().len(), 4097);
    assert!(store.records("recording").unwrap().is_empty());
    assert_no_pending(&directory);
    assert!(
        !fs::read_dir(store.root().join("artifacts"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".pending-"))
    );
}

#[test]
fn retention_bounds_top_level_enumeration_and_selected_file_count() {
    for limits in [
        Limits {
            entries: 0,
            ..Default::default()
        },
        Limits {
            files: 0,
            ..Default::default()
        },
    ] {
        let (_root, store, directory, mut document) = fixture();
        let error = retain(&store, &mut document, &directory, limits, || Ok(())).unwrap_err();
        assert!(error.to_string().contains("raw retention exceeds"));
        assert_eq!(document["raw_retention"]["complete"], false);
        assert_eq!(document["raw_retention"]["actual_read_bytes"], 0);
        assert!(store.artifacts("recording").unwrap().is_empty());
        assert_eq!(fs::read(directory.join("input.json")).unwrap(), b"input");
    }
}

#[test]
fn incomplete_metadata_encoding_keeps_prior_metadata_and_removes_its_owned_temporary_file() {
    let (_root, store, directory, mut document) = fixture();
    fs::write(directory.join("attempt.json"), b"prior evidence").unwrap();
    let error = retain(
        &store,
        &mut document,
        &directory,
        Limits {
            bytes: 4,
            ..Default::default()
        },
        || Ok(()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("metadata exceeds"));
    assert_eq!(
        fs::read(directory.join("attempt.json")).unwrap(),
        b"prior evidence"
    );
    assert_eq!(document["raw_retention"]["complete"], false);
    assert!(
        document["raw_retention"]["attempt_metadata_bytes"]
            .as_u64()
            .unwrap()
            <= 4
    );
    assert!(store.artifacts("recording").unwrap().is_empty());
    assert_no_pending(&directory);
}

#[test]
fn metadata_writer_checks_inside_a_large_serialization_unit() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("output");
    let mut count = 0;
    let mut checker = || {
        count += 1;
        anyhow::ensure!(count < 3, "stop within metadata");
        Ok(())
    };
    let mut written = 0;
    let mut output = MetadataWriter {
        file: fs::File::create(&file).unwrap(),
        maximum: 256 * 1024,
        written: &mut written,
        checkpoint: &mut checker,
    };
    let error = output.write_all(&vec![7; 192 * 1024]).unwrap_err();
    assert!(error.to_string().contains("stop within metadata"));
    drop(output);
    assert_eq!(written, 64 * 1024);
    assert_eq!(fs::metadata(file).unwrap().len(), written);
}

#[test]
fn a_failed_recovery_pass_preserves_known_prior_raw_references() {
    let (_root, store, directory, mut document) = fixture();
    let prior = store
        .add_artifact("recording", &directory.join("input.json"), "analysis-input")
        .unwrap();
    document["raw_artifacts"] = json!([prior]);
    let expected = document["raw_artifacts"].clone();
    let error = retain(&store, &mut document, &directory, Limits::default(), || {
        anyhow::bail!("recovery checkpoint expired")
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "recovery checkpoint expired");
    assert_eq!(document["raw_artifacts"], expected);
    assert_eq!(document["raw_retention"]["complete"], false);
    assert_eq!(
        fs::read(store.artifact_path("recording", &prior.id).unwrap()).unwrap(),
        b"input"
    );
}

#[test]
fn retention_checkpoint_interrupts_actual_staging_without_losing_workspace_evidence() {
    let (_root, store, directory, mut document) = fixture();
    fs::write(directory.join("stdout.jsonl"), vec![7; 256 * 1024]).unwrap();
    let staging = store.root().join("artifacts");
    let error = retain(&store, &mut document, &directory, Limits::default(), || {
        if fs::read_dir(&staging)?.any(|entry| {
            entry.is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with(".pending-"))
        }) {
            anyhow::bail!("original attempt checkpoint stopped staging");
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "original attempt checkpoint stopped staging"
    );
    assert_eq!(document["raw_retention"]["complete"], false);
    assert_eq!(document["raw_retention"]["actual_read_bytes"], 0);
    assert!(store.artifacts("recording").unwrap().is_empty());
    assert_eq!(
        fs::metadata(directory.join("stdout.jsonl")).unwrap().len(),
        256 * 1024
    );
    assert!(!fs::read_dir(staging).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".pending-")
    }));
}

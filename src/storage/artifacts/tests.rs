use super::*;
use serde_json::json;

fn store(directory: &Path) -> Store {
    let store = Store::open(directory).unwrap();
    store.save_run(&serde_json::from_value(json!({"id":"raw-budget","name":"raw","status":"completed","command":[],"cwd":".","runtime":"none","started_at_us":1,"finished_at_us":2,"exit_code":0,"diagnostics":[],"source":{},"correctness":{}})).unwrap()).unwrap();
    store
}

#[test]
fn controlled_staging_publishes_exact_bytes_and_refuses_a_grown_input() {
    let root = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    let store = store(root.path());
    let path = inputs.path().join("raw.bin");
    fs::write(&path, b"exact").unwrap();
    let mut read = 0;
    let artifact = store
        .add_artifact_controlled("raw-budget", &path, "raw", 5, &mut read, || Ok(()))
        .unwrap();
    assert_eq!(read, 5);
    assert_eq!(
        fs::read(store.artifact_path("raw-budget", &artifact.id).unwrap()).unwrap(),
        b"exact"
    );
    let mut grew = false;
    let error = store
        .add_artifact_controlled("raw-budget", &path, "raw", 5, &mut read, || {
            let staged = fs::read_dir(root.path().join("artifacts"))?
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().starts_with(".pending-"));
            if !grew && staged {
                fs::write(&path, vec![7; 1000])?;
                grew = true;
            }
            Ok(())
        })
        .expect_err("grown source must not publish a prefix");
    assert!(error.to_string().contains("byte limit"), "{error:#}");
    assert_eq!(read, 11, "exact source plus grown limit+one sentinel");
    assert_eq!(store.artifacts("raw-budget").unwrap().len(), 1);
    assert!(
        !fs::read_dir(root.path().join("artifacts"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".pending-"))
    );
}

#[test]
fn checkpoint_failure_after_a_real_chunk_cleans_partial_staging() {
    let root = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    let store = store(root.path());
    let path = inputs.path().join("raw.bin");
    fs::write(&path, vec![9; 150_000]).unwrap();
    let mut read = 0;
    let mut saw_partial = false;
    let error = store
        .add_artifact_controlled("raw-budget", &path, "raw", 200_000, &mut read, || {
            let partial = fs::read_dir(root.path().join("artifacts"))?
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with(".pending-"))
                .any(|entry| {
                    // Directory-entry size can lag writes through an open
                    // handle. Observe staged bytes rather than cached metadata.
                    fs::File::open(entry.path()).is_ok_and(|mut file| {
                        file.read(&mut [0u8; 1]).is_ok_and(|count| count == 1)
                    })
                });
            if partial {
                saw_partial = true;
                bail!("controlled deadline elapsed");
            }
            Ok(())
        })
        .expect_err("deadline checkpoint must prevent reference publication");
    assert!(saw_partial);
    assert!(error.to_string().contains("deadline"));
    assert!(
        read > 0 && read < 150_000,
        "actual source bytes read: {read}"
    );
    assert!(store.artifacts("raw-budget").unwrap().is_empty());
    assert_eq!(
        fs::read_dir(root.path().join("artifacts")).unwrap().count(),
        0
    );
    assert_eq!(fs::metadata(path).unwrap().len(), 150_000);
}

#[test]
fn controlled_staging_refuses_static_nonregular_sources_before_reading() {
    let root = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    let store = store(root.path());
    let mut read = 0;
    assert!(
        store
            .add_artifact_controlled(
                "raw-budget",
                inputs.path(),
                "raw",
                100,
                &mut read,
                || Ok(())
            )
            .unwrap_err()
            .to_string()
            .contains("regular file")
    );
    assert_eq!(read, 0);
    #[cfg(unix)]
    {
        let file = inputs.path().join("file");
        fs::write(&file, b"bytes").unwrap();
        let alias = inputs.path().join("alias");
        std::os::unix::fs::symlink(file, &alias).unwrap();
        assert!(
            store
                .add_artifact_controlled("raw-budget", &alias, "raw", 100, &mut read, || Ok(()))
                .is_err()
        );
        assert_eq!(read, 0);
    }
    assert!(store.artifacts("raw-budget").unwrap().is_empty());
}

#[test]
fn artifact_path_refuses_corrupt_retained_digest_metadata() {
    let root = tempfile::tempdir().unwrap();
    let store = store(root.path());
    let artifact = store
        .add_artifact_bytes("raw-budget", "raw.bin", "raw", b"bytes")
        .unwrap();
    let mut metadata = serde_json::to_value(&artifact).unwrap();
    metadata["sha256"] = json!("../../outside");
    store
        .connection()
        .unwrap()
        .execute(
            "UPDATE artifacts SET metadata=?1 WHERE run_id=?2 AND id=?3",
            params![metadata.to_string(), "raw-budget", artifact.id],
        )
        .unwrap();
    assert!(
        store
            .artifact_path("raw-budget", &artifact.id)
            .unwrap_err()
            .to_string()
            .contains("canonical SHA-256")
    );
}

#[test]
fn exact_artifact_metadata_cannot_redirect_to_a_different_indexed_blob() {
    let root = tempfile::tempdir().unwrap();
    let store = store(root.path());
    let original = store
        .add_artifact_bytes("raw-budget", "raw.bin", "raw", b"original")
        .unwrap();
    let unrelated = store
        .add_artifact_bytes("raw-budget", "other.bin", "raw", b"unrelated")
        .unwrap();
    let mut metadata = serde_json::to_value(&original).unwrap();
    metadata["sha256"] = json!(unrelated.sha256);
    store
        .connection()
        .unwrap()
        .execute(
            "UPDATE artifacts SET metadata=?1 WHERE run_id=?2 AND id=?3",
            params![metadata.to_string(), "raw-budget", original.id],
        )
        .unwrap();
    assert!(
        store
            .artifact_metadata("raw-budget", &original.id)
            .unwrap_err()
            .to_string()
            .contains("indexed")
    );
}

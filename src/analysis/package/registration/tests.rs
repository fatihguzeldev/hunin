use super::*;

fn registration_fixture() -> (tempfile::TempDir, Store, PathBuf, Value) {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("runner"), b"analyzer source").unwrap();
    let store = Store::open(&directory.path().join("store")).unwrap();
    let params = json!({"name":"inventory control","command":["./runner"],"cwd":source,"requirements":{"runtime":"fixture","source_fingerprint":"build-v1"}});
    (directory, store, source, params)
}

fn assert_unadmitted(store: &Store) {
    assert!(store.documents("analyses").unwrap().is_empty());
    assert_eq!(
        fs::read_dir(store.root().join("analysis-packages"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn registration_counts_empty_directories_and_cleans_its_failed_staging() {
    let (_directory, store, source, params) = registration_fixture();
    for index in 0..4 {
        fs::create_dir(source.join(format!("empty-{index}"))).unwrap();
    }
    let error = register_controlled(
        &store,
        &params,
        &InventoryLimits {
            entries: 2,
            ..Default::default()
        },
        || Ok(()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("2 entries"));
    assert_unadmitted(&store);
}

#[test]
fn registration_bounds_repeated_explicit_roots_even_when_files_are_deduplicated() {
    let (_directory, store, _source, mut params) = registration_fixture();
    params["files"] = json!(["runner", "runner", "runner"]);
    let error = register_controlled(
        &store,
        &params,
        &InventoryLimits {
            entries: 2,
            ..Default::default()
        },
        || Ok(()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("2 entries"));
    assert_unadmitted(&store);
}

#[test]
fn registration_limits_explicit_depth_and_cumulative_path_text() {
    let (_directory, store, source, mut params) = registration_fixture();
    fs::create_dir_all(source.join("one/two")).unwrap();
    fs::write(source.join("one/two/code.py"), b"source").unwrap();
    params["files"] = json!(["one/two/code.py"]);
    let error = register_controlled(
        &store,
        &params,
        &InventoryLimits {
            depth: 2,
            ..Default::default()
        },
        || Ok(()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("2 path components"));
    assert_unadmitted(&store);
    params["files"] = json!(["runner"]);
    let error = register_controlled(
        &store,
        &params,
        &InventoryLimits {
            path_bytes: 5,
            ..Default::default()
        },
        || Ok(()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("5 relative-path bytes"));
    assert_unadmitted(&store);
}

#[test]
fn a_registration_checkpoint_after_source_copy_cannot_publish_a_package() {
    let (_directory, store, _source, params) = registration_fixture();
    let root = store.root().join("analysis-packages");
    let error = register_controlled(&store, &params, &InventoryLimits::default(), || {
        if root.is_dir()
            && fs::read_dir(&root)?
                .any(|entry| entry.is_ok_and(|entry| entry.path().join("source/runner").is_file()))
        {
            anyhow::bail!("registration caller checkpoint stopped");
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "registration caller checkpoint stopped");
    assert_unadmitted(&store);
}

#[test]
fn default_registration_preserves_exclusions_digests_and_explicit_file_semantics() {
    let (_directory, store, source, params) = registration_fixture();
    fs::create_dir(source.join("node_modules")).unwrap();
    fs::write(source.join("node_modules/dependency"), b"excluded").unwrap();
    let first = register(&store, &params).unwrap();
    let second = register(&store, &params).unwrap();
    assert_eq!(
        first["analysis"]["package_digest"],
        second["analysis"]["package_digest"]
    );
    assert_eq!(first["analysis"]["files"].as_object().unwrap().len(), 1);
    let mut explicit = params;
    explicit["files"] = json!(["runner", "node_modules/dependency"]);
    let declared = register(&store, &explicit).unwrap();
    assert_eq!(declared["analysis"]["files"].as_object().unwrap().len(), 2);
}

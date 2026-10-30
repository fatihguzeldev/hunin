use super::*;
use serde_json::json;

fn legacy_package() -> (tempfile::TempDir, Value, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let managed = directory.path().canonicalize().unwrap();
    let source = managed.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("analyze.py"), b"print('legacy bytes')\n").unwrap();
    let executable = managed.join("external-tool");
    fs::write(&executable, b"external runtime identity").unwrap();
    // Size was not authoritative in older manifests; SHA remains enough.
    let analysis = json!({
        "package_dir":managed,
        "files":{"analyze.py":{"sha256":files::digest(b"print('legacy bytes')\n")}},
        "executable":{"path":executable,"sha256":files::digest(b"external runtime identity"),"packaged":false},
    });
    (directory, analysis, source)
}

#[test]
fn legacy_manifest_without_sizes_keeps_exact_verification_and_workspace_bytes() {
    let (directory, analysis, source) = legacy_package();
    assert_eq!(verify(&analysis, || Ok(())).unwrap(), source);
    let workspace = directory.path().join("workspace");
    copy_workspace(&analysis, &source, &workspace, || Ok(())).unwrap();
    assert_eq!(
        fs::read(workspace.join("analyze.py")).unwrap(),
        b"print('legacy bytes')\n"
    );
}

#[test]
fn old_manifest_cannot_bypass_the_existing_actual_source_byte_limit() {
    let (_directory, analysis, source) = legacy_package();
    // A sparse file tests the boundary without a large read or allocation.
    fs::File::create(source.join("analyze.py"))
        .unwrap()
        .set_len(MAX_BYTES + 1)
        .unwrap();
    let error = verify(&analysis, || Ok(())).unwrap_err();
    assert!(format!("{error:#}").contains("64 MiB"));
    let copy = tempfile::tempdir().unwrap();
    let error = copy_workspace(&analysis, &source, copy.path(), || Ok(())).unwrap_err();
    assert!(format!("{error:#}").contains("64 MiB"));
    assert_eq!(
        fs::metadata(copy.path().join("analyze.py")).unwrap().len(),
        0
    );
}

#[test]
fn old_manifest_cannot_bypass_the_existing_source_file_count_limit() {
    let (_directory, mut analysis, _) = legacy_package();
    analysis["files"] = Value::Object(
        (0..=MAX_FILES)
            .map(|index| (format!("file-{index}"), json!({"sha256":"unused"})))
            .collect(),
    );
    let error = verify(&analysis, || Ok(())).unwrap_err();
    assert!(error.to_string().contains("2000 files"));
}

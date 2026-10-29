use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, process::Command};

#[test]
fn build_info_uses_compiled_contracts_without_assets_or_store_access() {
    let root = tempfile::tempdir().unwrap();
    let moved = root.path().join("moved executable with spaces");
    fs::create_dir(&moved).unwrap();
    let binary = moved.join(if cfg!(windows) { "hunin.exe" } else { "hunin" });
    fs::copy(env!("CARGO_BIN_EXE_hunin"), &binary).unwrap();
    let forbidden_store = root.path().join("file-is-not-a-store");
    fs::write(&forbidden_store, b"existing user bytes").unwrap();
    let result = Command::new(&binary)
        .args(["--data-dir"])
        .arg(&forbidden_store)
        .arg("build-info")
        .current_dir(root.path())
        .env("HUNIN_ASSETS_DIR", root.path().join("absent-assets"))
        .env("HUNIN_UI_DIR", root.path().join("absent-ui"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    assert_eq!(fs::read(&forbidden_store).unwrap(), b"existing user bytes");
    let info: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(info["schema_version"], 1);
    assert_eq!(info["app"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(info["app"]["source_revision"], Value::Null);
    assert_eq!(
        info["storage"]["maximum_schema_version"],
        hunin::storage::SCHEMA_VERSION
    );
    for (name, bytes) in [
        (
            "record",
            include_bytes!("../protocol/record.schema.json").as_slice(),
        ),
        (
            "responses",
            include_bytes!("../protocol/response.schema.json").as_slice(),
        ),
        (
            "operations",
            include_bytes!("../protocol/operations.json").as_slice(),
        ),
        (
            "native_manifest",
            include_bytes!("../collectors/native/manifest.json").as_slice(),
        ),
    ] {
        assert_eq!(
            info["contracts"][name]["sha256"],
            format!("{:x}", Sha256::digest(bytes))
        );
    }
    assert_eq!(
        info["contracts"]["native_manifest"]["extension"]["flag"],
        "--lifetime-output"
    );
    assert!(!root.path().join("hunin.sqlite").exists());
    assert!(!root.path().join("artifacts").exists());
}

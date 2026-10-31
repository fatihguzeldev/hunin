use serde_json::Value;
use std::process::Command;

#[test]
fn cli_catalog_exposes_the_shared_schema_without_registering_a_project() {
    let root = tempfile::tempdir().unwrap();
    let registry = root.path().join("not-created");
    let output = Command::new(env!("CARGO_BIN_EXE_hunin"))
        .arg("--registry-dir")
        .arg(&registry)
        .args(["catalog", "analyses.register"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual: Value = serde_json::from_slice(&output.stdout).unwrap();
    let expected = hunin::operations::catalog()
        .iter()
        .find(|entry| entry["name"] == "analyses.register")
        .unwrap();
    assert_eq!(actual, *expected);
    assert!(!registry.exists());
    let help = Command::new(env!("CARGO_BIN_EXE_hunin"))
        .args(["call", "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&help.stdout).contains("hunin catalog"));
    let runtimes = Command::new(env!("CARGO_BIN_EXE_hunin"))
        .args(["run", "--help"])
        .output()
        .unwrap();
    for runtime in hunin::collection::registered_runtimes() {
        assert!(String::from_utf8_lossy(&runtimes.stdout).contains(runtime.id));
    }
}

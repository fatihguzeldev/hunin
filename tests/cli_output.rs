use serde_json::Value;
use std::process::Command;

#[test]
fn large_source_run_is_compact_and_exposes_complete_provenance_on_demand() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    for index in 0..5000 {
        std::fs::write(project.join(format!("source-{index:04}.py")), "pass\n").unwrap();
    }
    let binary = env!("CARGO_BIN_EXE_hunin");
    let store = root.path().join("store");
    let run = |full: bool| {
        let mut command = Command::new(binary);
        command
            .env_remove("HUNIN_PROJECT")
            .env_remove("HUNIN_REGISTRY_DIR");
        command
            .arg("--data-dir")
            .arg(&store)
            .args(["run", "--runtime", "none", "--cwd"])
            .arg(&project);
        if full {
            command.arg("--full");
        }
        let output = command
            .arg("--")
            .arg(binary)
            .arg("--version")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    let compact = run(false);
    let summary: Value = serde_json::from_slice(&compact).unwrap();
    assert_eq!(summary["format"], "hunin-recording-summary-v1");
    assert!(
        compact.len() < 32 * 1024,
        "compact output grew to {} bytes",
        compact.len()
    );
    assert_eq!(summary["run"]["source"]["source_files_count"], 5000);
    assert!(summary["run"]["source"].get("source_files").is_none());
    let inspection = summary["next"]["inspect"].as_array().unwrap();
    let full = Command::new(binary)
        .args(inspection.iter().skip(1).map(|v| v.as_str().unwrap()))
        .output()
        .unwrap();
    assert!(full.status.success());
    let full: Value = serde_json::from_slice(&full.stdout).unwrap();
    assert_eq!(full["run"]["id"], summary["run"]["id"]);
    assert_eq!(
        full["run"]["source"]["source_files"]
            .as_object()
            .unwrap()
            .len(),
        5000
    );
    assert_eq!(
        full["run"]["source"]["source_snapshot_complete"],
        summary["run"]["source"]["source_snapshot_complete"]
    );
    assert_eq!(
        full["run"]["source"]["collection_status"],
        summary["run"]["source"]["collection_status"]
    );
    let explicit: Value = serde_json::from_slice(&run(true)).unwrap();
    assert!(explicit.get("format").is_none());
    assert_eq!(
        explicit["run"]["source"]["source_files"]
            .as_object()
            .unwrap()
            .len(),
        5000
    );
}

#[test]
fn compact_project_run_exposes_an_inspection_command_that_keeps_its_registry() {
    let root = tempfile::tempdir().unwrap();
    let registry = root.path().join("custom registry");
    let project = root.path().join("work");
    std::fs::create_dir(&project).unwrap();
    let binary = env!("CARGO_BIN_EXE_hunin");
    let output = Command::new(binary)
        .env_remove("HUNIN_DATA_DIR")
        .env_remove("HUNIN_PROJECT")
        .arg("--registry-dir")
        .arg(&registry)
        .args(["run", "--runtime", "none", "--cwd"])
        .arg(&project)
        .arg("--")
        .arg(binary)
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(summary["project"]["id"].is_string());
    let command = summary["next"]["inspect"].as_array().unwrap();
    assert!(command.iter().any(|v| *v == summary["project"]["id"]));
    let inspected = Command::new(binary)
        .args(command.iter().skip(1).map(|v| v.as_str().unwrap()))
        .output()
        .unwrap();
    assert!(
        inspected.status.success(),
        "{}",
        String::from_utf8_lossy(&inspected.stderr)
    );
    let inspected: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(inspected["run"]["id"], summary["run"]["id"]);
}

use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hunin"));
    for key in ["HUNIN_PROJECT", "HUNIN_DATA_DIR", "HUNIN_REGISTRY_DIR"] {
        command.env_remove(key);
    }
    command
}

fn successful(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn selected(root: &Path, project: &str, operation: &str, params: Value) -> Value {
    successful(
        cli()
            .arg("--registry-dir")
            .arg(root)
            .args(["--project", project, "call", operation])
            .arg(params.to_string())
            .output()
            .unwrap(),
    )
}

#[test]
fn capabilities_discovery_does_not_create_a_project_or_store() {
    let temp = tempfile::tempdir().unwrap();
    let registry = temp.path().join("uncreated-registry");
    for arguments in [vec!["capabilities"], vec!["call", "capabilities", "{}"]] {
        let capabilities = successful(
            cli()
                .arg("--registry-dir")
                .arg(&registry)
                .current_dir(temp.path())
                .args(arguments)
                .output()
                .unwrap(),
        );
        assert!(capabilities["import_formats"].is_array());
        assert!(
            !registry.exists(),
            "host discovery must not register its working directory"
        );
    }
}

#[test]
fn cli_and_mcp_select_independent_stores_and_retain_imported_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("empty.jsonl");
    std::fs::write(&input, "").unwrap();
    let mut projects = Vec::new();
    for name in ["alpha", "beta"] {
        let created = successful(
            cli()
                .arg("--registry-dir")
                .arg(root.path())
                .args(["projects", "create", name])
                .output()
                .unwrap(),
        );
        let project = created["project"].clone();
        let imported = selected(
            root.path(),
            name,
            "recordings.import",
            json!({"path":input,"format":"hunin-jsonl","name":name}),
        );
        let run_id = imported["run"]["id"].as_str().unwrap();
        assert_eq!(imported["run"]["name"], name);
        let artifacts = selected(
            root.path(),
            project["id"].as_str().unwrap(),
            "artifacts.list",
            json!({"run_id":run_id}),
        );
        assert!(!artifacts["artifacts"].as_array().unwrap().is_empty());
        let store = Path::new(project["store_path"].as_str().unwrap());
        assert!(store.join("hunin.sqlite").is_file());
        assert!(
            std::fs::read_dir(store.join("artifacts"))
                .unwrap()
                .next()
                .is_some()
        );
        projects.push((project, run_id.to_owned()));
    }
    assert_ne!(projects[0].0["id"], projects[1].0["id"]);
    assert_ne!(projects[0].0["store_path"], projects[1].0["store_path"]);
    for (index, (project, run_id)) in projects.iter().enumerate() {
        let runs = selected(
            root.path(),
            project["name"].as_str().unwrap(),
            "runs.list",
            json!({}),
        );
        assert_eq!(runs["runs"].as_array().unwrap().len(), 1);
        assert_eq!(runs["runs"][0]["id"], *run_id);
        assert_ne!(runs["runs"][0]["id"], projects[1 - index].1);

        let mut child = cli()
            .arg("--registry-dir")
            .arg(root.path())
            .args(["--project", project["id"].as_str().unwrap(), "mcp"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        for request in [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"project-control","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"runs.list","arguments":{}}}),
        ] {
            writeln!(input, "{request}").unwrap();
        }
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let replies: Vec<Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let listed = replies.iter().find(|reply| reply["id"] == 2).unwrap();
        assert_ne!(listed["result"]["isError"], true);
        assert_eq!(
            listed["result"]["structuredContent"]["runs"][0]["id"],
            *run_id
        );
        assert_eq!(
            listed["result"]["structuredContent"]["runs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}

#[test]
fn legacy_store_remains_explicit_and_unknown_selection_never_creates_a_replacement() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("old.jsonl");
    std::fs::write(&input, "").unwrap();
    let imported = successful(
        cli()
            .arg("--data-dir")
            .arg(root.path())
            .args(["import", input.to_str().unwrap(), "--format", "hunin-jsonl"])
            .output()
            .unwrap(),
    );
    let listed = successful(
        cli()
            .arg("--registry-dir")
            .arg(root.path())
            .args(["projects", "list"])
            .output()
            .unwrap(),
    );
    assert_eq!(listed["projects"].as_array().unwrap().len(), 1);
    assert_eq!(listed["projects"][0]["id"], "legacy");
    assert_eq!(listed["projects"][0]["legacy"], true);
    assert_eq!(
        selected(root.path(), "legacy", "runs.list", json!({}))["runs"][0]["id"],
        imported["run"]["id"]
    );
    let unknown = cli()
        .arg("--registry-dir")
        .arg(root.path())
        .args(["--project", "missing", "list"])
        .output()
        .unwrap();
    assert!(!unknown.status.success());
    assert!(unknown.stdout.is_empty());
    let after = successful(
        cli()
            .arg("--registry-dir")
            .arg(root.path())
            .args(["projects", "list"])
            .output()
            .unwrap(),
    );
    assert_eq!(after, listed);
    let direct = successful(
        cli()
            .arg("--data-dir")
            .arg(root.path())
            .arg("list")
            .output()
            .unwrap(),
    );
    assert_eq!(direct["runs"][0]["id"], imported["run"]["id"]);
}

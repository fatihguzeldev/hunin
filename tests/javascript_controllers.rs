use serde_json::Value;
use std::{
    path::Path,
    process::{Command, Output},
};

fn node() -> String {
    let output = Command::new("node")
        .args(["-p", "process.execPath"])
        .env_remove("NODE_OPTIONS")
        .env_remove("BUN_OPTIONS")
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn record_without_global_node(
    runtime: &str,
    executable: &Path,
    arguments: &[String],
    cwd: &Path,
    expected: &str,
    option: Option<(&str, &str)>,
) -> (Output, Value) {
    let data = tempfile::tempdir().unwrap();
    let empty_path = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_hunin"));
    command
        .args(["--data-dir"])
        .arg(data.path())
        .args(["run", "--runtime", runtime, "--cwd"])
        .arg(cwd)
        .args(["--expected-stdout", expected, "--timeout", "10", "--"])
        .arg(executable)
        .args(arguments)
        .current_dir(cwd)
        .env("PATH", empty_path.path())
        .env_remove("HUNIN_NODE")
        .env_remove("NODE_OPTIONS")
        .env_remove("BUN_OPTIONS")
        .env("HUNIN_ASSETS_DIR", env!("CARGO_MANIFEST_DIR"));
    if let Some((key, value)) = option {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    let result: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
    (output, result["run"].clone())
}

#[test]
fn selected_node_controls_capture_without_global_node_and_preserves_target_options() {
    let project = tempfile::tempdir().unwrap();
    let executable = project.path().join(if cfg!(windows) {
        "selected-node.exe"
    } else {
        "selected-node"
    });
    #[cfg(unix)]
    std::os::unix::fs::symlink(node(), &executable).unwrap();
    #[cfg(windows)]
    std::fs::copy(node(), &executable).unwrap();
    let preload = project.path().join("inherited options.cjs");
    std::fs::write(&preload, "console.log('inherited-target');").unwrap();
    let options = format!(
        "--require={}",
        serde_json::to_string(preload.to_string_lossy().as_ref()).unwrap()
    );
    for exit in [0, 7] {
        let arguments = vec![
            "-e".into(),
            format!("console.log('target-ok');process.exit({exit});"),
        ];
        let (output, run) = record_without_global_node(
            "node",
            &executable,
            &arguments,
            project.path(),
            "inherited-target\ntarget-ok\n",
            Some(("NODE_OPTIONS", &options)),
        );
        assert_eq!(run["exit_code"], exit, "{run}");
        assert_eq!(output.status.success(), exit == 0, "{run}");
        assert_eq!(run["source"]["collection_status"], "completed", "{run}");
        assert_eq!(
            run["source"]["launch_executable"],
            executable.to_string_lossy().as_ref()
        );
        assert_eq!(
            run["source"]["collector_settings"]["command"][0],
            executable.to_string_lossy().as_ref()
        );
    }
}

#[test]
#[ignore = "requires HUNIN_TEST_BUN"]
fn selected_bun_controls_capture_without_node_or_project_configuration_leakage() {
    let executable = std::env::var("HUNIN_TEST_BUN").expect("set HUNIN_TEST_BUN");
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("inherited.ts"),
        "console.log('inherited-target');",
    )
    .unwrap();
    std::fs::write(
        project.path().join("configured.ts"),
        "console.log('configured-target');",
    )
    .unwrap();
    std::fs::write(
        project.path().join("bunfig.toml"),
        "preload = [\"./configured.ts\"]\n",
    )
    .unwrap();
    std::fs::write(
        project.path().join(".env"),
        "HUNIN_CONTROLLER_DOTENV_TEST=loaded-for-target\n",
    )
    .unwrap();
    let target = project.path().join("target.ts");
    std::fs::write(
        &target,
        "console.log('target-ok:'+process.env.HUNIN_CONTROLLER_DOTENV_TEST);",
    )
    .unwrap();
    let direct = Command::new(&executable)
        .arg(&target)
        .current_dir(project.path())
        .env_remove("NODE_OPTIONS")
        .env("BUN_OPTIONS", "--preload=./inherited.ts")
        .output()
        .unwrap();
    assert!(
        direct.status.success(),
        "{}",
        String::from_utf8_lossy(&direct.stderr)
    );
    let expected = String::from_utf8(direct.stdout).unwrap();
    assert!(expected.contains("inherited-target") && expected.contains("loaded-for-target"));
    let (output, run) = record_without_global_node(
        "bun",
        Path::new(&executable),
        &[target.to_string_lossy().into_owned()],
        project.path(),
        &expected,
        Some(("BUN_OPTIONS", "--preload=./inherited.ts")),
    );
    assert!(output.status.success(), "{run}");
    assert_eq!(run["source"]["collection_status"], "completed");
    assert_eq!(run["command"][0], executable);
}

#[test]
#[ignore = "requires HUNIN_TEST_DENO"]
fn selected_deno_controls_capture_without_node_and_keeps_target_permissions() {
    let executable = std::env::var("HUNIN_TEST_DENO").expect("set HUNIN_TEST_DENO");
    let project = tempfile::tempdir().unwrap();
    let target = project.path().join("target.ts");
    std::fs::write(&target, "console.log('target-ok');").unwrap();
    let arguments = vec![
        "run".into(),
        "--deny-sys".into(),
        target.to_string_lossy().into_owned(),
    ];
    let (output, run) = record_without_global_node(
        "deno",
        Path::new(&executable),
        &arguments,
        project.path(),
        "target-ok\n",
        None,
    );
    assert!(output.status.success(), "{run}");
    assert_eq!(run["source"]["collection_status"], "completed");
    let command = run["source"]["collector_settings"]["command"]
        .as_array()
        .unwrap();
    assert!(command.iter().any(|argument| argument == "--deny-sys"));
    assert!(!command.iter().any(|argument| {
        [
            "--allow-read",
            "--allow-write",
            "--allow-env",
            "--allow-run",
        ]
        .iter()
        .any(|flag| argument == flag)
    }));
}

#[test]
#[cfg(unix)]
fn unavailable_selected_controller_is_not_replaced_with_a_global_node() {
    use std::os::unix::fs::PermissionsExt;
    let project = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let fake = project.path().join("unavailable-controller");
    let marker = project.path().join("target-ran");
    std::fs::write(&fake, "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'v22.16.0\\n'; exit 0; fi\nif [ \"$1\" = -e ]; then touch target-ran; exit 23; fi\nexit 17\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hunin"))
        .arg("--data-dir")
        .arg(data.path())
        .args(["run", "--runtime", "node", "--cwd"])
        .arg(project.path())
        .arg("--")
        .arg(&fake)
        .args(["-e", "console.log('target')"])
        .env("HUNIN_NODE", node())
        .env("HUNIN_ASSETS_DIR", env!("CARGO_MANIFEST_DIR"))
        .env_remove("NODE_OPTIONS")
        .env_remove("BUN_OPTIONS")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        result["run"]["exit_code"].is_null(),
        "controller failure must not be recorded as target exit: {result}"
    );
    assert!(
        !marker.exists(),
        "target must not run when its selected controller is unavailable"
    );
}

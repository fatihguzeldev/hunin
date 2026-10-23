use hunin::{execution::Engine, storage::Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, process::Command};

fn python() -> String {
    std::env::var("HUNIN_TEST_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into())
}

fn output(command: &mut Command) -> String {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[tokio::test]
async fn recording_preserves_real_virtual_environment_for_absolute_relative_and_path_launches() {
    let project = tempfile::tempdir().unwrap();
    let cwd = project.path().canonicalize().unwrap();
    let venv = cwd.join("venv with space");
    output(
        Command::new(python())
            .args(["-m", "venv", "--without-pip"])
            .arg(&venv),
    );
    let relative = Path::new("venv with space").join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    });
    let launcher = cwd.join(&relative);
    let inspection: Value = serde_json::from_str(&output(Command::new(&launcher).args([
        "-c",
        "import json,sys,sysconfig; print(json.dumps({'prefix':sys.prefix,'base_prefix':sys.base_prefix,'purelib':sysconfig.get_path('purelib')}))",
    ]))).unwrap();
    assert_ne!(inspection["prefix"], inspection["base_prefix"]);
    std::fs::write(
        Path::new(inspection["purelib"].as_str().unwrap()).join("hunin_venv_marker.py"),
        "VALUE = 'only installed in the selected virtual environment'\n",
    )
    .unwrap();
    let code = "import json,sys,hunin_venv_marker; print(json.dumps({'prefix':sys.prefix,'base_prefix':sys.base_prefix,'executable':sys.executable,'marker':hunin_venv_marker.VALUE}))";
    let physical = launcher.canonicalize().unwrap();
    let expected_hash = format!("{:x}", Sha256::digest(std::fs::read(&physical).unwrap()));
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    let engine = Engine::new(store.clone());
    let cases = [
        (launcher.display().to_string(), json!({}), launcher.clone()),
        (relative.display().to_string(), json!({}), launcher.clone()),
        (
            "python".into(),
            json!({"PATH":relative.parent().unwrap(),"PATHEXT":".EXE;.CMD;.BAT;.COM"}),
            if cfg!(windows) {
                launcher.with_extension("EXE")
            } else {
                launcher.clone()
            },
        ),
    ];
    #[cfg(unix)]
    let cases = {
        let mut cases = cases.to_vec();
        // Case-only aliases exercise sys.executable spelling on this host too.
        // Python's prefix/package environment must remain the same for both.
        for name in ["python.exe", "python.EXE"] {
            let alias = launcher.parent().unwrap().join(name);
            if !alias.exists() {
                std::os::unix::fs::symlink("python", &alias).unwrap();
            }
            cases.push((alias.display().to_string(), json!({}), alias));
        }
        cases
    };
    for (name, env, direct_launcher) in cases {
        // PATHEXT can select python.EXE while the directory entry spells .exe.
        // Compare with a direct launch using that exact same path spelling.
        let expected = output(Command::new(&direct_launcher).args(["-c", code]));
        let direct: Value = serde_json::from_str(&expected).unwrap();
        assert_eq!(direct["prefix"], inspection["prefix"]);
        assert_eq!(direct["base_prefix"], inspection["base_prefix"]);
        let started = engine.start(serde_json::from_value(json!({"command":[name,"-c",code],"cwd":cwd,"runtime":"none","env":env,"expected_stdout":expected})).unwrap()).await.unwrap();
        let run = engine.wait(&started.id).await.unwrap();
        let stdout = store
            .artifacts(&run.id)
            .unwrap()
            .into_iter()
            .find(|artifact| artifact.name == "stdout.txt")
            .unwrap();
        let actual =
            std::fs::read_to_string(store.artifact_path(&run.id, &stdout.id).unwrap()).unwrap();
        if let Some(directory) = std::env::var_os("HUNIN_EXECUTABLE_LAUNCH_EVIDENCE") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join(format!("{}.json",run.id)),serde_json::to_vec_pretty(&json!({"command":name,"direct_launcher":direct_launcher,"expected_stdout":expected,"actual_stdout":actual,"run":run})).unwrap()).unwrap();
        }
        assert_eq!(
            actual, expected,
            "{name}, recorded launch: {}",
            run.source["launch_executable"]
        );
        assert_eq!(run.status, "completed", "{name}: {:?}", run.diagnostics);
        assert_eq!(
            run.correctness["passed"], true,
            "{name}: {:?}",
            run.correctness
        );
        assert_eq!(
            Path::new(run.source["executable"].as_str().unwrap()),
            physical
        );
        assert_eq!(run.source["executable_sha256"], expected_hash);
        assert_eq!(
            Path::new(run.source["launch_executable"].as_str().unwrap()),
            direct_launcher
        );
    }
}

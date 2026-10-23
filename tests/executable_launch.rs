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
    let expected = output(Command::new(&launcher).args(["-c", code]));
    let physical = launcher.canonicalize().unwrap();
    let expected_hash = format!("{:x}", Sha256::digest(std::fs::read(&physical).unwrap()));
    let data = tempfile::tempdir().unwrap();
    let engine = Engine::new(Store::open(data.path()).unwrap());
    for (name, env) in [
        (launcher.display().to_string(), json!({})),
        (relative.display().to_string(), json!({})),
        (
            "python".into(),
            json!({"PATH":relative.parent().unwrap(),"PATHEXT":".EXE;.CMD;.BAT;.COM"}),
        ),
    ] {
        let started = engine.start(serde_json::from_value(json!({"command":[name,"-c",code],"cwd":cwd,"runtime":"none","env":env,"expected_stdout":expected})).unwrap()).await.unwrap();
        let run = engine.wait(&started.id).await.unwrap();
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
            launcher
        );
    }
}

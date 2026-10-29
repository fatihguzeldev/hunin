use hunin::collection::{CaptureRequest, prepare};
use serde_json::Value;
use std::{collections::BTreeMap, fs, process::Command};

#[test]
#[ignore = "requires the selected GraalPy executable in HUNIN_TEST_GRAAL"]
fn graal_native_entry_forms_preserve_main_lifetime_and_runtime_semantics() {
    let executable = std::env::var("HUNIN_TEST_GRAAL").expect("set HUNIN_TEST_GRAAL");
    let directory = tempfile::tempdir().unwrap();
    let work = directory.path();
    // Observe builtins and locals before calling exec or a helper: Graal's
    // native script/-c launcher and its runpy route differ at this boundary.
    let source = r#"import atexit, json, os, sys, time
initial_builtins = type(__builtins__).__name__
initial_locals_are_globals = locals() is globals()
marker = 'native target namespace'
def observe(stage):
    import __main__
    loader = globals().get('__loader__')
    print(json.dumps({
        'stage': stage, 'pid': os.getpid(),
        'same_main': vars(__main__) is globals(),
        'marker': getattr(__main__, 'marker', None),
        'initial_builtins': initial_builtins,
        'initial_locals_are_globals': initial_locals_are_globals,
        'argv': sys.argv, 'cwd': os.getcwd(), 'path0': sys.path[0],
        'file': globals().get('__file__'), 'package': __package__,
        'cached': globals().get('__cached__'),
        'spec': __spec__.name if __spec__ else None,
        'loader': getattr(loader, '__name__', type(loader).__name__),
        'loader_path': getattr(loader, 'path', None),
    }, sort_keys=True))
def entry_hotspot():
    deadline = time.monotonic() + 0.2
    value = 0
    while time.monotonic() < deadline:
        value += 1
    return value
atexit.register(observe, 'atexit')
observe('target')
entry_hotspot()
raise SystemExit(7)
"#;
    fs::write(work.join("main.py"), source).unwrap();
    fs::create_dir(work.join("target_package")).unwrap();
    fs::write(work.join("target_package/__init__.py"), "").unwrap();
    fs::write(work.join("target_package/main.py"), source).unwrap();
    fs::write(work.join("target_package/__main__.py"), source).unwrap();
    fs::create_dir(work.join("bundle")).unwrap();
    fs::write(work.join("bundle/__main__.py"), source).unwrap();
    let setup = Command::new(&executable)
        .args(["-c", "import py_compile, zipfile; py_compile.compile('main.py', 'entry.pyc', doraise=True)\nwith zipfile.ZipFile('bundle.pyz', 'w') as archive: archive.write('main.py', '__main__.py')"])
        .current_dir(work)
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stderr)
    );
    for (label, target) in [
        ("script", vec!["main.py"]),
        ("module", vec!["-m", "target_package.main"]),
        ("package", vec!["-m", "target_package"]),
        ("command", vec!["-c", source]),
        ("directory", vec!["bundle"]),
        ("zip", vec!["bundle.pyz"]),
        ("bytecode", vec!["entry.pyc"]),
    ] {
        let arguments = target
            .into_iter()
            .chain(["argument with spaces"])
            .collect::<Vec<_>>();
        let native = Command::new(&executable)
            .args(&arguments)
            .current_dir(work)
            .output()
            .unwrap();
        let request = CaptureRequest {
            command: std::iter::once(executable.clone())
                .chain(arguments.iter().map(|arg| (*arg).into()))
                .collect(),
            cwd: work.into(),
            artifact_dir: work.join(format!("capture-{label}")),
            run_id: format!("graal-{label}"),
            runtime: "graalpy".into(),
            env: BTreeMap::new(),
        };
        let plan = prepare(&request).unwrap();
        assert!(
            plan.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("target stdout"))
        );
        let child = Command::new(&plan.command[0])
            .args(&plan.command[1..])
            .envs(&plan.env)
            .current_dir(work)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        plan.on_started(&request, pid).unwrap();
        let captured = child.wait_with_output().unwrap();
        assert_eq!(captured.status.code(), native.status.code(), "{label}");
        assert_eq!(captured.stderr, native.stderr, "{label}");
        let stdout = String::from_utf8(captured.stdout).unwrap();
        let diagnostic = format!(
            "Printing output to {}",
            request.artifact_dir.join("cpu.graal.json").display()
        );
        // The runtime itself emits this diagnostic using its host line ending;
        // separating it for this comparison does not establish byte-exact
        // target stdout parity or strip production capture evidence.
        let target_stdout = stdout
            .strip_suffix(&format!("{diagnostic}\r\n"))
            .or_else(|| stdout.strip_suffix(&format!("{diagnostic}\n")))
            .expect("CPUSampler output-file diagnostic");
        let native_stdout = String::from_utf8(native.stdout).unwrap();
        let observations = |value: &str, expected_pid: Option<u32>| {
            value
                .lines()
                .map(|line| {
                    let mut observation: Value = serde_json::from_str(line).unwrap();
                    if let Some(pid) = expected_pid {
                        assert_eq!(observation["pid"], pid);
                    }
                    observation.as_object_mut().unwrap().remove("pid");
                    observation
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            observations(target_stdout, Some(pid)),
            observations(&native_stdout, None),
            "{label}"
        );
        let bundle = plan.finalize(&request).unwrap();
        bundle.completion.ensure_complete().unwrap();
        if label != "bytecode" {
            assert_eq!(native.status.code(), Some(7), "{label}");
            assert_eq!(observations(target_stdout, Some(pid)).len(), 2, "{label}");
            let imported = hunin::importers::import(
                &bundle.profiles[0].path,
                "graal-cpusampler",
                &request.run_id,
            )
            .unwrap();
            assert!(
                imported
                    .records
                    .iter()
                    .any(|record| record["kind"] == "sample"
                        && record["pid"] == pid
                        && record["stack"].as_array().is_some_and(|stack| stack
                            .iter()
                            .any(|frame| frame["function"] == "entry_hotspot"))),
                "{label}: expected actual hotspot and supervised PID"
            );
        }
    }
}

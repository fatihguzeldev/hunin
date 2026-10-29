use hunin::{model::runs::Run, storage::Store};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{OnceLock, mpsc},
    time::Duration,
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn executable(variable: &str, default: &str) -> OsString {
    std::env::var_os(variable).unwrap_or_else(|| default.into())
}
fn fixture(name: &str) -> OsString {
    root().join("sdk/conformance").join(name).into_os_string()
}

struct Supervised(Child);
impl Drop for Supervised {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
fn command(arguments: &[OsString], directory: &Path, enabled: bool) -> Command {
    let mut target = Command::new(&arguments[0]);
    target.args(&arguments[1..]);
    target.env("HUNIN_RUN_ID", if enabled { "sdk-contract" } else { "" });
    target.env(
        "HUNIN_RECORD_DIR",
        if enabled {
            directory.as_os_str()
        } else {
            std::ffi::OsStr::new("")
        },
    );
    target.env("PYTHONPATH", root().join("sdk/python"));
    target.env_remove("HUNIN_SDK_TOKEN");
    target
}
fn execute_pair(arguments: &[OsString], directory: &Path, enabled: bool) {
    let mut parent = Supervised(
        command(arguments, directory, enabled)
            .env("HUNIN_SDK_FIXTURE_MODE", "parent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start SDK fixture; install the runtime or set HUNIN_TEST_* explicitly"),
    );
    let stdout = parent.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut sent = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !sent && let Some((_, token)) = line.split_once("HUNIN_TOKEN ") {
                let _ = sender.send(Some(token.to_owned()));
                sent = true;
            }
            // Drain the remaining harness/target output while the parent waits.
        }
        if !sent {
            let _ = sender.send(None);
        }
    });
    let token = receiver
        .recv_timeout(Duration::from_secs(30))
        .expect("SDK fixture did not yield its propagation token");
    let token: Value = serde_json::from_str(
        token
            .as_deref()
            .expect("SDK fixture exited before token emission"),
    )
    .unwrap();
    let mut remote_command = command(arguments, directory, enabled);
    if !arguments.iter().any(|arg| arg == "--exact") {
        remote_command.arg("--child");
    }
    let remote = remote_command
        .env("HUNIN_SDK_FIXTURE_MODE", "child")
        .env("HUNIN_SDK_TOKEN", token.to_string())
        .env(
            "HUNIN_SDK_PARENT_RUN",
            token["run_id"].as_str().unwrap_or(""),
        )
        .env(
            "HUNIN_SDK_PARENT_CONTEXT",
            token["context_id"].as_str().unwrap_or(""),
        )
        .env(
            "HUNIN_SDK_PARENT_ID",
            token["parent_id"].as_str().unwrap_or(""),
        )
        .output()
        .expect("start propagated SDK process");
    assert!(
        remote.status.success(),
        "SDK child failed: {}",
        String::from_utf8_lossy(&remote.stderr)
    );
    let remote_id = String::from_utf8_lossy(&remote.stdout)
        .lines()
        .find_map(|line| {
            line.split_once("HUNIN_REMOTE ")
                .map(|(_, value)| value.to_owned())
        })
        .expect("SDK child did not identify its span");
    writeln!(parent.0.stdin.take().unwrap(), "{remote_id}").unwrap();
    let status = parent.0.wait().unwrap();
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut parent.0.stderr.take().unwrap(), &mut stderr).unwrap();
    assert!(status.success(), "SDK parent failed: {stderr}");
}
fn records(directory: &Path) -> Vec<Value> {
    if !directory.exists() {
        return vec![];
    }
    fs::read_dir(directory)
        .unwrap()
        .flat_map(|entry| {
            let path = entry.unwrap().path();
            assert_eq!(path.extension().and_then(|s| s.to_str()), Some("jsonl"));
            fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect::<Vec<Value>>()
        })
        .collect()
}
fn verify_sdk(arguments: Vec<OsString>, cell: &str) {
    eprintln!("Verifying actual SDK runtime cell: {cell}");
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("enabled");
    execute_pair(&arguments, &directory, true);
    let mut rows = records(&directory);
    let spans: HashMap<_, _> = rows
        .iter()
        .filter(|r| r["kind"] == "span")
        .map(|r| (r["name"].as_str().unwrap(), r))
        .collect();
    assert_eq!(
        spans.len(),
        6,
        "six named spans, including a separate process, must exist"
    );
    assert_eq!(rows.iter().filter(|r| r["kind"] == "context").count(), 1);
    assert_eq!(rows.iter().filter(|r| r["kind"] == "metric").count(), 3);
    assert_eq!(rows.iter().filter(|r| r["kind"] == "relation").count(), 1);
    for name in ["a", "b"] {
        assert_eq!(spans[name]["parent_id"], spans["root"]["id"]);
        assert_eq!(
            spans[&*format!("{name}-child")]["parent_id"],
            spans[name]["id"]
        );
        assert!(
            spans[&*format!("{name}-child")]["duration_us"]
                .as_u64()
                .unwrap()
                <= spans[name]["duration_us"].as_u64().unwrap()
        );
    }
    let remote = spans["remote"];
    assert_eq!(remote["parent_id"], spans["root"]["id"]);
    assert_ne!(remote["producer_id"], spans["root"]["producer_id"]);
    assert_ne!(remote["pid"], spans["root"]["pid"]);
    let expected_context = spans["root"]["context_id"].as_str().unwrap();
    for span in spans.values() {
        assert_eq!(span["context_id"], expected_context);
    }
    for row in &rows {
        assert_eq!(row["basis"], "explicit");
        assert!(
            row["attributes"]["thread_id_source"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "thread namespace missing: {row}"
        );
        assert!(
            row["attributes"]["clock"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "clock source missing: {row}"
        );
    }
    let producer_ids: HashSet<_> = rows
        .iter()
        .map(|r| r["producer_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        producer_ids.len(),
        2,
        "producer identity must be per-process lifetime"
    );
    let child_only: Vec<_> = rows
        .iter()
        .filter(|r| r["producer_id"] == remote["producer_id"])
        .cloned()
        .collect();
    let store = Store::open(&temporary.path().join("store")).unwrap();
    store
        .save_run(&Run {
            id: "sdk-contract".into(),
            name: cell.into(),
            status: "completed".into(),
            command: arguments
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect(),
            cwd: root().display().to_string(),
            runtime: cell.into(),
            started_at_us: 0,
            finished_at_us: Some(9_007_199_254_740_991),
            exit_code: Some(0),
            diagnostics: vec![],
            source: json!({"fixture":"sdk/conformance"}),
            correctness: json!({"passed":true}),
        })
        .unwrap();
    assert!(
        store
            .accept("sdk-contract", "incomplete-child-only", &child_only)
            .is_err()
    );
    assert!(
        store.records("sdk-contract").unwrap().is_empty(),
        "failed cross-process reference batch must publish nothing"
    );
    // Submit descendants before their ancestors and context declaration. Acceptance
    // checks the whole result set, rather than relying on spool ordering.
    rows.sort_by_key(|r| std::cmp::Reverse(r["timestamp_us"].as_u64().unwrap()));
    assert!(
        store
            .accept("sdk-contract", "complete-cross-process", &rows)
            .unwrap()
    );
    assert!(
        !store
            .accept("sdk-contract", "complete-cross-process", &rows)
            .unwrap()
    );
    assert_eq!(store.records("sdk-contract").unwrap().len(), rows.len());
    let disabled = temporary.path().join("disabled");
    execute_pair(&arguments, &disabled, false);
    assert!(
        records(&disabled).is_empty(),
        "disabled SDK produced a recording"
    );
}

#[test]
fn javascript_node_records_conform_and_propagate_across_processes() {
    verify_sdk(
        vec![
            executable("HUNIN_TEST_NODE", "node"),
            fixture("javascript.mjs"),
        ],
        "node",
    );
}
#[test]
fn python_records_conform_and_propagate_across_processes() {
    verify_sdk(
        vec![
            executable(
                "HUNIN_TEST_PYTHON",
                if cfg!(windows) { "python" } else { "python3" },
            ),
            fixture("python.py"),
        ],
        "cpython",
    );
}
#[test]
fn go_records_conform_and_propagate_across_processes() {
    let build = tempfile::tempdir().unwrap();
    let path = build.path().join(if cfg!(windows) {
        "go-sdk-fixture.exe"
    } else {
        "go-sdk-fixture"
    });
    let status = Command::new(executable("HUNIN_TEST_GO", "go"))
        .current_dir(root().join("sdk/go"))
        .args([
            OsString::from("build"),
            OsString::from("-o"),
            path.clone().into_os_string(),
            fixture("go.go"),
        ])
        .status()
        .expect("Go SDK integration requires Go");
    assert!(status.success());
    verify_sdk(vec![path.into_os_string()], "go");
}
static NATIVE_FIXTURES: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
fn native_fixture(name: &str) -> OsString {
    let (_, build) = NATIVE_FIXTURES.get_or_init(|| {
        let temporary = tempfile::tempdir().unwrap();
        let cmake = executable("HUNIN_TEST_CMAKE", "cmake");
        assert!(
            Command::new(&cmake)
                .args([
                    OsString::from("-S"),
                    fixture(""),
                    OsString::from("-B"),
                    temporary.path().into(),
                    OsString::from("-DCMAKE_BUILD_TYPE=Release")
                ])
                .status()
                .expect("C/C++ SDK integration requires CMake and compilers")
                .success()
        );
        assert!(
            Command::new(cmake)
                .arg("--build")
                .arg(temporary.path())
                .args(["--config", "Release"])
                .status()
                .unwrap()
                .success()
        );
        let directory = if cfg!(windows) {
            temporary.path().join("Release")
        } else {
            temporary.path().to_path_buf()
        };
        (temporary, directory)
    });
    build
        .join(format!(
            "hunin_sdk_{name}_fixture{}",
            if cfg!(windows) { ".exe" } else { "" }
        ))
        .into_os_string()
}
#[test]
fn c_records_conform_and_propagate_across_processes() {
    verify_sdk(vec![native_fixture("c")], "c");
}
#[test]
fn cpp_records_conform_and_propagate_across_processes() {
    verify_sdk(vec![native_fixture("cpp")], "cpp");
}
#[test]
fn rust_records_conform_and_propagate_across_processes() {
    verify_sdk(
        vec![
            std::env::current_exe().unwrap().into_os_string(),
            "--ignored".into(),
            "--exact".into(),
            "rust_sdk_fixture_child".into(),
            "--nocapture".into(),
        ],
        "rust",
    );
}

#[test]
#[ignore = "Explicit compatibility cells: supply all four official runtime paths; an ignored cell is not a support result"]
fn additional_runtime_cells_conform_with_explicit_runtime_paths() {
    for (variable, cell, program) in [
        ("HUNIN_TEST_BUN", "bun", "javascript.mjs"),
        ("HUNIN_TEST_DENO", "deno", "javascript.mjs"),
        ("HUNIN_TEST_PYPY", "pypy", "python.py"),
        ("HUNIN_TEST_GRAALPY", "graalpy", "python.py"),
    ] {
        verify_additional_runtime(variable, cell, program);
    }
}

fn verify_additional_runtime(variable: &str, cell: &str, program: &str) {
    let executable = std::env::var_os(variable)
        .unwrap_or_else(|| panic!("{variable} is required; unavailable runtimes are not verified"));
    let mut arguments = vec![executable];
    if cell == "deno" {
        arguments.extend(["run".into(), "--allow-all".into()]);
    }
    arguments.push(fixture(program));
    verify_sdk(arguments, cell);
}

#[test]
#[ignore = "Requires explicit HUNIN_TEST_BUN; unavailable runtimes are not verified"]
fn bun_runtime_sdk_conforms() {
    verify_additional_runtime("HUNIN_TEST_BUN", "bun", "javascript.mjs");
}
#[test]
#[ignore = "Requires explicit HUNIN_TEST_DENO; unavailable runtimes are not verified"]
fn deno_runtime_sdk_conforms() {
    verify_additional_runtime("HUNIN_TEST_DENO", "deno", "javascript.mjs");
}
#[test]
#[ignore = "Requires explicit HUNIN_TEST_PYPY; unavailable runtimes are not verified"]
fn pypy_runtime_sdk_conforms() {
    verify_additional_runtime("HUNIN_TEST_PYPY", "pypy", "python.py");
}
#[test]
#[ignore = "Requires explicit HUNIN_TEST_GRAALPY; unavailable runtimes are not verified"]
fn graalpy_runtime_sdk_conforms() {
    verify_additional_runtime("HUNIN_TEST_GRAALPY", "graalpy", "python.py");
}

#[test]
#[ignore = "Helper subprocess entrypoint, invoked by rust_records_conform_and_propagate_across_processes"]
fn rust_sdk_fixture_child() {
    use hunin_sdk::{Context, counter, flush, relation};
    if std::env::var("HUNIN_SDK_FIXTURE_MODE").as_deref() == Ok("child") {
        let token: Context =
            serde_json::from_str(&std::env::var("HUNIN_SDK_TOKEN").unwrap()).unwrap();
        let remote = token.span("remote");
        counter(&token, "jobs", 1.0, "count");
        println!("HUNIN_REMOTE {}", remote.id.as_deref().unwrap_or(""));
        remote.end();
    } else {
        let context = Context::new("request");
        let parent = context.span("root");
        let token = parent.context();
        let workers: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|name| {
                let token = token.clone();
                std::thread::spawn(move || {
                    let branch = token.span(name);
                    let nested = branch.context();
                    let _child = nested.span(&format!("{name}-child"));
                    counter(&nested, "jobs", 1.0, "count");
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        println!("HUNIN_TOKEN {}", serde_json::to_string(&token).unwrap());
        std::io::stdout().flush().unwrap();
        let mut remote = String::new();
        std::io::stdin().read_line(&mut remote).unwrap();
        if let Some(parent_id) = parent.id.as_ref().filter(|_| !remote.trim().is_empty()) {
            relation(&token, parent_id, remote.trim(), "dispatches", &[]);
        }
        parent.end();
    }
    flush().unwrap();
}

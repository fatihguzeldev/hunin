use crate::{
    collection::{CaptureOutput, CapturePlan, CaptureRequest},
    host::{assets, probe},
    model::calls::CountRequest,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, process::Command, time::Duration};

pub(super) fn limitations() -> Vec<String> {
    ["Requires CPython 3.12+, matching development headers and a native C compiler; the callback is built only in Hunin's managed local cache", "Python code starts and generator/coroutine resumptions in the main GIL interpreter; native C builtins, child processes, subinterpreters and free-threaded builds are outside this inventory", "All code callbacks run; selectors filter aggregation rather than reducing callback coverage", "Count-table or output bounds and missing finalization make collection incomplete; absent functions are not synthesized as zero"].map(str::to_owned).into()
}

pub(super) fn prerequisite(request: &CaptureRequest) -> Result<Value> {
    super::super::python::arguments(&request.command[1..])?;
    let output = probe::standard(
        Command::new(&request.command[0])
            .envs(&request.env)
            .current_dir(&request.cwd)
            .arg(assets().join("collectors/counts/build.py"))
            .arg("--probe"),
    )?;
    ensure!(
        output.status.success(),
        "Python counting prerequisites unavailable: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).context("read Python counting prerequisite identity")
}

pub(super) fn prepare(request: &CaptureRequest, options: &CountRequest) -> Result<CapturePlan> {
    let cache = request
        .artifact_dir
        .parent()
        .and_then(|path| path.parent())
        .context("count capture needs a managed artifact parent")?
        .join("collector-cache/counts-python");
    let output = probe::output(
        Command::new(&request.command[0])
            .envs(&request.env)
            .current_dir(&request.cwd)
            .arg(assets().join("collectors/counts/build.py"))
            .arg(&cache),
        None,
        Duration::from_secs(40),
        128 * 1024,
    )?;
    ensure!(
        output.status.success(),
        "Python counting callback unavailable: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let built: Value = serde_json::from_slice(&output.stdout)
        .context("read native count callback build identity")?;
    let path = request.artifact_dir.join("count-config.json");
    fs::write(
        &path,
        serde_json::to_vec(
            &json!({"run_id":request.run_id,"output":request.artifact_dir.join("calls.calls.jsonl"),"completion":request.artifact_dir.join("calls.complete.json"),"extension":built["extension"],"functions":options.functions,"max_functions":options.max_functions,"max_output_bytes":options.max_output_bytes}),
        )?,
    )?;
    let (flags, target) = super::super::python::arguments(&request.command[1..])?;
    let mut command = vec![request.command[0].clone()];
    command.extend(flags);
    command.extend([
        assets()
            .join("collectors/counts/python.py")
            .to_string_lossy()
            .into_owned(),
        path.to_string_lossy().into_owned(),
        "--".into(),
    ]);
    command.extend(target);
    Ok(CapturePlan {
        command,
        env: BTreeMap::new(),
        diagnostics: limitations(),
        runtime_version: built["identity"]["runtime"].as_str().map(str::to_owned),
        collector_identity: Some(json!({"name":"cpython-monitoring-native","callback":built})),
        session_identity: BTreeMap::new(),
        output: CaptureOutput::FunctionCounts {
            method: "cpython-monitoring-native".into(),
            maximum_bytes: options.max_output_bytes,
        },
    })
}

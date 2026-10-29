use super::{CaptureOutput, CapturePlan, CaptureRequest};
use crate::host::{assets, probe};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub(super) const GRAAL_COMPLETION_SCOPE: &str = "declared_graal_profile_and_owned_process_identity";
const GRAAL_PROFILE: &str = "cpu.graal.json";
const GRAAL_METADATA: &str = "cpu.graal.meta.json";
const GRAAL_PID_SOURCE: &str = "supervised_runtime_process_spawn";

pub(super) fn graal_started(
    request: &CaptureRequest,
    pid: u32,
    version: Option<&str>,
) -> Result<()> {
    ensure!(pid > 0, "Graal runtime process PID is unavailable");
    let executable = request
        .command
        .first()
        .context("Graal runtime launch is missing")?;
    let path = request.artifact_dir.join(GRAAL_METADATA);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options
        .open(&path)
        .with_context(|| format!("retain owned Graal process identity in {}", path.display()))?;
    serde_json::to_writer(
        &mut output,
        &json!({
            "schema_version":1,"kind":"hunin-graal-process-v1","run_id":request.run_id,
            "pid":pid,"pid_source":GRAAL_PID_SOURCE,"launch_executable":executable,
            "runtime_version_probe":version,
        }),
    )?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    Ok(())
}

#[derive(Deserialize)]
struct GraalProcess {
    schema_version: u64,
    kind: String,
    run_id: String,
    pid: u64,
    pid_source: String,
    launch_executable: String,
}

pub(super) fn validate_graal_output(request: &CaptureRequest, files: &[PathBuf]) -> Result<()> {
    let metadata = request.artifact_dir.join(GRAAL_METADATA);
    ensure!(
        files.contains(&request.artifact_dir.join(GRAAL_PROFILE)),
        "planned Graal CPUSampler profile is missing: {GRAAL_PROFILE}"
    );
    let mut bytes = Vec::new();
    fs::File::open(&metadata)
        .with_context(|| {
            format!(
                "planned Graal process identity is missing: {}",
                metadata.display()
            )
        })?
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 64 * 1024,
        "Graal process identity exceeds 64 KiB"
    );
    let process: GraalProcess =
        serde_json::from_slice(&bytes).context("invalid Graal process identity")?;
    ensure!(
        process.schema_version == 1
            && process.kind == "hunin-graal-process-v1"
            && process.run_id == request.run_id
            && process.pid > 0
            && process.pid <= u64::from(u32::MAX)
            && process.pid_source == GRAAL_PID_SOURCE
            && request.command.first() == Some(&process.launch_executable),
        "Graal process identity is not bound to the planned runtime launch"
    );
    Ok(())
}

pub(crate) fn cpu_prerequisite(executable: &Path, timeout: Duration) -> Result<()> {
    let output = probe::output(
        Command::new(executable)
            .args([
                "-c",
                r"import sys, threading
sys.path.insert(0, sys.argv[1])
from hunin_capture.cpu_progress.clocks import clock
assert hasattr(threading.current_thread(), 'native_id'), 'Python native thread IDs required'
counter = clock()
try:
    assert counter.snapshot({threading.current_thread().native_id}), 'OS thread CPU counter unavailable'
finally:
    counter.close()",
            ])
            .arg(assets().join("collectors/python")),
        None,
        timeout,
        32 * 1024,
    )
    .with_context(|| format!("probe Python CPU counters with {}", executable.display()))?;
    if !output.status.success() {
        bail!(
            "Python CPU progress prerequisites unavailable: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

pub(super) fn vmprof(request: &CaptureRequest) -> Result<CapturePlan> {
    let interpreter = &request.command[0];
    let check = probe::standard(Command::new(interpreter)
        .envs(&request.env)
        .current_dir(&request.cwd)
        .args(["-c", "import vmprof,sys,importlib.metadata as m; v=m.version('vmprof'); assert tuple(map(int,v.split('.')[:2])) == (0,6), 'Hunin requires the vmprof 0.6 series'; print(sys.version+'\\nvmprof '+v)"]))
        .with_context(|| format!("launch Python interpreter {interpreter}"))?;
    if !check.status.success() {
        bail!(
            "{interpreter} needs vmprof 0.6 in this interpreter (`{interpreter} -m pip install 'vmprof>=0.6,<0.7'`). The interpreter is never silently replaced. {}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
    let (flags, target) = arguments(&request.command[1..])?;
    let mut command = vec![interpreter.clone()];
    command.extend(flags);
    command.extend([
        assets()
            .join("collectors/python/capture.py")
            .display()
            .to_string(),
        request
            .artifact_dir
            .join("cpu.vmprof")
            .display()
            .to_string(),
        "--".into(),
    ]);
    command.extend(target);
    Ok(CapturePlan {
        command,
        env: BTreeMap::new(),
        collector_identity: None,
        session_identity: BTreeMap::new(),
        output: CaptureOutput::Profiles {
            format: ("vmprof").into(),
        },
        runtime_version: Some(String::from_utf8_lossy(&check.stdout).trim().to_owned()),
        diagnostics: vec![
            "vmprof captures Python threads in one process; child processes require separate collection".into(),
            "CPU-clock sample times are retained without an invented wall-clock mapping".into(),
        ],
    })
}

pub(super) fn cpu_progress(request: &CaptureRequest) -> Result<CapturePlan> {
    let check = probe::standard(Command::new(&request.command[0])
        .envs(&request.env)
        .current_dir(&request.cwd)
        .args(["-c", "import sys,threading; assert hasattr(threading.current_thread(),'native_id'), 'Python native thread IDs required'; print(sys.version)"]))
        .context("inspect Python CPU sampling prerequisites")?;
    if !check.status.success() {
        bail!(
            "Python CPU progress sampling unavailable in requested interpreter: {}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
    let (flags, target) = arguments(&request.command[1..])?;
    let mut command = vec![request.command[0].clone()];
    command.extend(flags);
    command.extend([
        assets()
            .join("collectors/python/cpu.py")
            .display()
            .to_string(),
        request
            .artifact_dir
            .join("cpu.pythoncpu.jsonl")
            .display()
            .to_string(),
        "--".into(),
    ]);
    command.extend(target);
    Ok(CapturePlan {
        command,
        env: BTreeMap::new(),
        collector_identity: None,
        session_identity: BTreeMap::new(),
        output: CaptureOutput::Profiles {
            format: ("python-cpu-progress").into(),
        },
        runtime_version: Some(String::from_utf8_lossy(&check.stdout).trim().to_string()),
        diagnostics: vec![
            "Python stacks are gated and weighted by measured OS thread CPU progress across bounded observation windows; weights are statistical attribution, not individual call durations. Delayed observations remain unattributed CPU evidence.".into(),
            "GIL scheduling, native-only threads and short-lived undiscovered threads limit CPU attribution coverage; raw counters, frame snapshots and timing uncertainty are retained.".into(),
        ],
    })
}

pub(super) fn graal(request: &CaptureRequest) -> Result<CapturePlan> {
    let (flags, target) = arguments(&request.command[1..])?;
    let mut command = vec![request.command[0].clone()];
    command.extend(flags);
    command.extend([
        "--cpusampler".into(),
        "--cpusampler.Output=json".into(),
        format!(
            "--cpusampler.OutputFile={}",
            request.artifact_dir.join(GRAAL_PROFILE).display()
        ),
        "--cpusampler.GatherHitTimes=true".into(),
    ]);
    command.extend(target);
    Ok(CapturePlan {
        command,
        env: BTreeMap::new(),
        collector_identity: Some(json!({
            "kind":"graalpy-cpusampler","measurement_contract":"graal-direct-entry-v1",
            "completion_scope":GRAAL_COMPLETION_SCOPE,
        })),
        session_identity: BTreeMap::new(),
        output: CaptureOutput::GraalCpuSampler,
        runtime_version: probe::standard(Command::new(&request.command[0])
            .envs(&request.env)
            .current_dir(&request.cwd)
            .arg("--version"))
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()),
        diagnostics: vec![
            "GraalPy distribution must expose Truffle CPUSampler; native-image and JVM-hosted modes differ".into(),
            "CPU sampler call-tree weights are aggregated; no per-call duration is inferred".into(),
            "Graal CPUSampler appends its output-file diagnostic to target stdout; exact stdout checks include that runtime output".into(),
        ],
    })
}

fn arguments(args: &[String]) -> Result<(Vec<String>, Vec<String>)> {
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "-c" || argument == "-m" || !argument.starts_with('-') {
            break;
        }
        if ["-u", "-O", "-OO", "-B", "-s", "-q", "-v"].contains(&argument.as_str())
            || argument == "--jvm"
            || argument.starts_with("--vm.")
            || argument.starts_with("--python.")
        {
            index += 1;
        } else if argument == "-W" || argument == "-X" {
            if args.get(index + 1).is_none() {
                bail!("{argument} requires a value");
            }
            index += 2;
        } else {
            bail!(
                "Python collector does not support interpreter option {argument}; isolated/no-site/stdin modes require a separate collection route"
            );
        }
    }
    if index >= args.len() {
        bail!("Python collection requires a script, -m module or -c code");
    }
    Ok((args[..index].to_vec(), args[index..].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::CompletionState;

    fn graal_request(directory: &Path) -> CaptureRequest {
        CaptureRequest {
            command: vec!["selected-graal-launcher".into(), "main.py".into()],
            cwd: directory.into(),
            artifact_dir: directory.into(),
            run_id: "selected-graal-run".into(),
            runtime: "graalpy".into(),
            env: BTreeMap::new(),
        }
    }

    fn graal_plan() -> CapturePlan {
        serde_json::from_value(json!({
            "command":[],"runtime_version":"prelaunch version evidence",
            "output":{"kind":"graal-cpu-sampler"},
        }))
        .unwrap()
    }

    #[test]
    fn graal_spawn_retains_only_observed_identity_and_never_overwrites_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let request = graal_request(directory.path());
        let plan = graal_plan();
        plan.on_started(&request, 123).unwrap();
        let path = directory.path().join(GRAAL_METADATA);
        let bytes = fs::read(&path).unwrap();
        let metadata: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(metadata["run_id"], request.run_id);
        assert_eq!(metadata["pid"], 123);
        assert_eq!(metadata["pid_source"], GRAAL_PID_SOURCE);
        assert_eq!(metadata["launch_executable"], request.command[0]);
        assert_eq!(
            metadata["runtime_version_probe"],
            "prelaunch version evidence"
        );
        assert!(metadata.get("sys.version").is_none());
        assert!(metadata.get("interpreter").is_none());
        assert!(metadata.get("epoch_start_us").is_none());
        assert!(plan.on_started(&request, 456).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn graal_completion_requires_the_exact_profile_and_same_launch_identity() {
        let directory = tempfile::tempdir().unwrap();
        let request = graal_request(directory.path());
        let plan = graal_plan();
        fs::write(directory.path().join("unrelated.graal.json"), b"{}").unwrap();
        assert!(matches!(
            plan.finalize(&request).unwrap().completion.state,
            CompletionState::Incomplete
        ));
        let profile = directory.path().join(GRAAL_PROFILE);
        fs::write(&profile, b"{}").unwrap();
        assert!(validate_graal_output(&request, std::slice::from_ref(&profile)).is_err());
        plan.on_started(&request, 123).unwrap();
        assert!(matches!(
            plan.finalize(&request).unwrap().completion.state,
            CompletionState::Complete
        ));
        let metadata = directory.path().join(GRAAL_METADATA);
        let original: serde_json::Value =
            serde_json::from_slice(&fs::read(&metadata).unwrap()).unwrap();
        for (field, invalid) in [
            ("schema_version", json!(2)),
            ("kind", json!("other")),
            ("run_id", json!("another-run")),
            ("pid", json!(0)),
            ("pid", json!(u64::from(u32::MAX) + 1)),
            ("pid_source", json!("guessed")),
            ("launch_executable", json!("another-launcher")),
        ] {
            let mut changed = original.clone();
            changed[field] = invalid;
            fs::write(&metadata, serde_json::to_vec(&changed).unwrap()).unwrap();
            assert!(
                validate_graal_output(&request, std::slice::from_ref(&profile)).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn other_collectors_have_no_spawn_sidecar_requirement() {
        let directory = tempfile::tempdir().unwrap();
        let request = graal_request(directory.path());
        let mut plan = graal_plan();
        plan.output = CaptureOutput::Unspecified;
        plan.on_started(&request, 0).unwrap();
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

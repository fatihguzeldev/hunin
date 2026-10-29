use super::{CaptureOutput, CapturePlan, CaptureRequest, assets, probe};
use anyhow::{Context, Result, bail};
use std::{collections::BTreeMap, path::Path, process::Command, time::Duration};

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
            request.artifact_dir.join("cpu.graal.json").display()
        ),
        "--cpusampler.GatherHitTimes=true".into(),
        assets()
            .join("collectors/python/graal.py")
            .display()
            .to_string(),
        request
            .artifact_dir
            .join("cpu.graal.meta.json")
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
            format: ("graal-cpusampler").into(),
        },
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

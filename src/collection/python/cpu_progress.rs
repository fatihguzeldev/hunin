use super::arguments;
use crate::{
    collection::{CaptureOutput, CapturePlan, CaptureRequest},
    host::{assets, probe},
};
use anyhow::{Context, Result, bail};
use std::{collections::BTreeMap, path::Path, process::Command, time::Duration};

pub(crate) fn prerequisite(executable: &Path, timeout: Duration) -> Result<()> {
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

pub(in crate::collection) fn prepare(request: &CaptureRequest) -> Result<CapturePlan> {
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

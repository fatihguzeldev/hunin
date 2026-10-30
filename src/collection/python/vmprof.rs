use super::arguments;
use crate::{
    collection::{CaptureOutput, CapturePlan, CaptureRequest},
    host::{assets, probe},
};
use anyhow::{Context, Result, bail};
use std::{collections::BTreeMap, process::Command};

pub(in crate::collection) fn prepare(request: &CaptureRequest) -> Result<CapturePlan> {
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

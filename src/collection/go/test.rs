use crate::collection::{CaptureOutput, CapturePlan, CaptureRequest};
use crate::host::probe;
use anyhow::{Result, bail};
use std::{collections::BTreeMap, fs, process::Command};

pub(in crate::collection) fn prepare(request: &CaptureRequest, trace: bool) -> Result<CapturePlan> {
    if request.command.get(1).map(String::as_str) != Some("test") {
        bail!("go-test/go-trace runtime expects `go test <one package> [test flags]`");
    }
    let mut command = request.command[..2].to_vec();
    command.push(format!(
        "-o={}",
        request
            .artifact_dir
            .join(if cfg!(windows) {
                "workload.test.exe"
            } else {
                "workload.test"
            })
            .display()
    ));
    command.push(format!(
        "{}={}",
        if trace { "-trace" } else { "-cpuprofile" },
        request
            .artifact_dir
            .join(if trace { "cpu.gotrace" } else { "cpu.pprof" })
            .display()
    ));
    command.extend_from_slice(&request.command[2..]);
    fs::write(
        request.artifact_dir.join("go-tool.json"),
        serde_json::to_vec(&serde_json::json!({"executable":request.command[0]}))?,
    )?;
    Ok(CapturePlan {
        command,
        env: BTreeMap::new(),
        collector_identity: None,
        session_identity: BTreeMap::new(),
        output: CaptureOutput::Profiles {
            format: (if trace { "go-trace" } else { "pprof" }).into(),
        },
        runtime_version: probe::standard(
            Command::new(&request.command[0])
                .envs(&request.env)
                .current_dir(&request.cwd)
                .arg("version"),
        )
        .ok()
        .filter(|o| o.status.success())
        .map(|r| String::from_utf8_lossy(&r.stdout).trim().to_owned()),
        diagnostics: vec![
            "Go test profiling uses one package per recording; CPU profile and execution trace are separate collection modes to avoid unnecessary profiler interference".into(),
        ],
    })
}

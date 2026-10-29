#[cfg(any(windows, test))]
mod cleanup;
mod outcome;
pub(crate) mod tool;

#[cfg(windows)]
pub(crate) use cleanup::{request_stop, wait as wait_for_cleanup};
pub(crate) use outcome::{read as read_target_outcome, unknown as unknown_target_outcome};

use super::{CaptureAdapter, CaptureOutput, CapturePlan, CaptureRequest, select_runtime};
use anyhow::{Context, Result};
use std::collections::BTreeMap;

pub(crate) fn handles_runtime(runtime: &str) -> bool {
    select_runtime(runtime).is_ok_and(|selected| selected.adapter == CaptureAdapter::Native)
}

pub(super) fn prepare(request: &CaptureRequest) -> Result<CapturePlan> {
    let samply = tool::resolve().context("native/Go sampling requires Hunin's bundled samply0.13.1-hunin-native1; reinstall collector assets or set HUNIN_SAMPLY to a patched executable (an explicit override must be an existing file)")?;
    let sampler_version = tool::check(&samply, None, std::time::Duration::from_secs(10))?;
    let collector_identity = tool::identity(&samply, &sampler_version)?;
    let mut command = vec![
        samply.display().to_string(),
        "record".into(),
        "--save-only".into(),
        "--unstable-presymbolicate".into(),
        "--lifetime-output".into(),
        request
            .artifact_dir
            .join("cpu.samply.lifetimes.json")
            .display()
            .to_string(),
        "-o".into(),
        request
            .artifact_dir
            .join("cpu.samply.json")
            .display()
            .to_string(),
    ];
    if cfg!(windows) {
        command.push("--keep-etl".into());
    }
    command.push("--".into());
    command.extend(request.command.clone());
    Ok(CapturePlan {
        command,
        env: BTreeMap::new(),
        collector_identity: Some(collector_identity),
        session_identity: BTreeMap::new(),
        output: CaptureOutput::Profiles { format: "firefox".into() },
        runtime_version: None,
        diagnostics: vec![
            "Native symbols/source require debug information for the measured build".into(),
            "Target runtime version is separate from collector identity. Go reports build information from the launched executable when a Go inspection tool is available; generic native executables and subprocess toolchain versions remain unknown.".into(),
            "Linux perf permissions, Windows ETW elevation, and macOS signing restrictions are checked by samply at capture time".into(),
            "OS samples do not imply Go goroutine/scheduler/GC tracing; import an enabled runtime pprof recording for that additional route".into(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::handles_runtime;

    #[test]
    fn native_finishing_follows_registered_mechanisms_and_preserves_aliases() {
        for runtime in ["go", "rust", "c", "cpp", "C++", "NATIVE"] {
            assert!(handles_runtime(runtime), "{runtime}");
        }
        for runtime in [
            "go-test",
            "go-trace",
            "node",
            "cpython",
            "python-cpu",
            "none",
            "unknown",
        ] {
            assert!(!handles_runtime(runtime), "{runtime}");
        }
    }
}

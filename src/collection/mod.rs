mod bun_producers;
mod deno_workers;
mod finalization;
mod go;
mod javascript;
pub(crate) mod native_tool;
mod python;
mod selection;
mod settings;
use anyhow::{Context, Result, bail};
pub use finalization::{
    CaptureArtifact, CaptureCompletion, CaptureOutput, CaptureProducer, CompletionState,
    RawCaptureBundle, RawProfile, capture_artifacts,
};
pub(crate) use python::cpu_prerequisite as python_cpu_prerequisite;
pub use selection::{
    CaptureAdapter, CaptureModeRegistration, LaunchExecutableKind, RuntimeRegistration,
    capture_modes, registered_runtimes, select_runtime,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureRequest {
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub artifact_dir: PathBuf,
    pub run_id: String,
    pub runtime: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturePlan {
    pub command: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub diagnostics: Vec<String>,
    pub runtime_version: Option<String>,
    #[serde(default)]
    pub collector_identity: Option<serde_json::Value>,
    /// Adapter-generated transient values, keyed by their stable semantic role.
    #[serde(default)]
    pub session_identity: BTreeMap<String, String>,
    #[serde(default)]
    pub output: CaptureOutput,
}

pub fn prepare(request: &CaptureRequest) -> Result<CapturePlan> {
    if request.command.is_empty() {
        bail!("a target command is required");
    }
    let selected = select_runtime(&request.runtime)?;
    let mut canonical = request.clone();
    canonical.runtime = selected.id.into();
    fs::create_dir_all(&canonical.artifact_dir)?;
    match selected.adapter {
        CaptureAdapter::JavaScript => javascript::prepare(&canonical),
        CaptureAdapter::PythonVmprof => python::vmprof(&canonical),
        CaptureAdapter::PythonCpuProgress => python::cpu_progress(&canonical),
        CaptureAdapter::GraalCpuSampler => python::graal(&canonical),
        CaptureAdapter::GoTestCpu => go::test(&canonical, false),
        CaptureAdapter::GoTrace => go::test(&canonical, true),
        CaptureAdapter::Native if canonical.runtime == "go" => go::native(&canonical),
        CaptureAdapter::Native => native(&canonical),
    }
}

pub(crate) fn profile_format(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_string_lossy();
    if name.ends_with(".pythoncpu.jsonl") {
        Some("python-cpu-progress")
    } else if name.ends_with(".cpuprofile") {
        Some("v8-cpuprofile")
    } else if name.ends_with(".vmprof") {
        Some("vmprof")
    } else if name.ends_with(".samply.json") || name.ends_with(".samply.json.gz") {
        Some("firefox")
    } else if name.ends_with(".graal.json") {
        Some("graal-cpusampler")
    } else if name.ends_with(".pprof") || name.ends_with(".pprof.gz") {
        Some("pprof")
    } else if name.ends_with(".gotrace") {
        Some("go-trace")
    } else {
        None
    }
}

fn native(request: &CaptureRequest) -> Result<CapturePlan> {
    let samply = native_tool::resolve().context("native/Go sampling requires Hunin's bundled samply0.13.1-hunin-native1; reinstall collector assets or set HUNIN_SAMPLY to a patched executable (an explicit override must be an existing file)")?;
    let sampler_version = native_tool::check(&samply, None, std::time::Duration::from_secs(10))?;
    let collector_identity = native_tool::identity(&samply, &sampler_version)?;
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
    Ok(CapturePlan { command, env:BTreeMap::new(), collector_identity:Some(collector_identity), session_identity:BTreeMap::new(),output:CaptureOutput::Profiles{format:("firefox").into()}, runtime_version: None, diagnostics:vec![
        "Native symbols/source require debug information for the measured build".into(),
        "Target runtime version is separate from collector identity. Go reports build information from the launched executable when a Go inspection tool is available; generic native executables and subprocess toolchain versions remain unknown.".into(),
        "Linux perf permissions, Windows ETW elevation, and macOS signing restrictions are checked by samply at capture time".into(),
        "OS samples do not imply Go goroutine/scheduler/GC tracing; import an enabled runtime pprof recording for that additional route".into()] })
}

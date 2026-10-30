mod finalization;
mod go;
mod javascript;
pub(crate) mod native;
mod python;
mod selection;
mod settings;
use anyhow::{Result, bail};
pub(crate) use finalization::artifacts_from_paths;
pub use finalization::{
    CaptureArtifact, CaptureCompletion, CaptureOutput, CaptureProducer, CompletionState,
    RawCaptureBundle, RawProfile, capture_artifacts,
};
pub(crate) use python::cpu_progress::prerequisite as python_cpu_prerequisite;
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
        CaptureAdapter::PythonVmprof => python::vmprof::prepare(&canonical),
        CaptureAdapter::PythonCpuProgress => python::cpu_progress::prepare(&canonical),
        CaptureAdapter::GraalCpuSampler => python::graal::prepare(&canonical),
        CaptureAdapter::GoTestCpu => go::test::prepare(&canonical, false),
        CaptureAdapter::GoTrace => go::test::prepare(&canonical, true),
        CaptureAdapter::Native if canonical.runtime == "go" => go::native::prepare(&canonical),
        CaptureAdapter::Native => native::prepare(&canonical),
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

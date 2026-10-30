//! Collector selection and adapter composition over one raw-delivery contract.
mod artifacts;
mod contract;
mod finalization;
mod go;
mod javascript;
pub(crate) mod native;
mod python;
mod selection;
mod settings;

use anyhow::{Result, bail};
pub(crate) use artifacts::artifacts_from_paths;
pub use artifacts::capture_artifacts;
pub use contract::{
    CaptureArtifact, CaptureCompletion, CaptureOutput, CapturePlan, CaptureProducer,
    CaptureRequest, CompletionState, RawCaptureBundle, RawProfile,
};
pub(crate) use python::cpu_progress::prerequisite as python_cpu_prerequisite;
pub use selection::{
    CaptureAdapter, CaptureModeRegistration, LaunchExecutableKind, RuntimeRegistration,
    capture_modes, registered_runtimes, select_runtime,
};
use std::fs;

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

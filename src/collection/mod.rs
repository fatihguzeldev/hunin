mod bun_producers;
mod deno_workers;
mod finalization;
mod javascript;
pub(crate) mod native_tool;
mod python;
mod selection;
mod settings;
use crate::host::{probe, tool};
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
    env, fs,
    path::{Path, PathBuf},
    process::Command,
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
        CaptureAdapter::GoTestCpu => go_test(&canonical, false),
        CaptureAdapter::GoTrace => go_test(&canonical, true),
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

fn go_test(request: &CaptureRequest, trace: bool) -> Result<CapturePlan> {
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
    Ok(CapturePlan{command,env:BTreeMap::new(),collector_identity:None,session_identity:BTreeMap::new(),output:CaptureOutput::Profiles{format:(if trace { "go-trace" } else { "pprof" }).into()},runtime_version:probe::standard(Command::new(&request.command[0]).envs(&request.env).current_dir(&request.cwd).arg("version")).ok().filter(|o|o.status.success()).map(|r|String::from_utf8_lossy(&r.stdout).trim().to_owned()),diagnostics:vec!["Go test profiling uses one package per recording; CPU profile and execution trace are separate collection modes to avoid unnecessary profiler interference".into()]})
}

fn native(request: &CaptureRequest) -> Result<CapturePlan> {
    let samply = native_tool::resolve().context("native/Go sampling requires Hunin's bundled samply0.13.1-hunin-native1; reinstall collector assets or set HUNIN_SAMPLY to a patched executable (an explicit override must be an existing file)")?;
    let sampler_version = native_tool::check(&samply, None, std::time::Duration::from_secs(10))?;
    let collector_identity = native_tool::identity(&samply, &sampler_version)?;
    let runtime_version = native_runtime_version(request);
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
    Ok(CapturePlan { command, env:BTreeMap::new(), collector_identity:Some(collector_identity), session_identity:BTreeMap::new(),output:CaptureOutput::Profiles{format:("firefox").into()}, runtime_version, diagnostics:vec![
        "Native symbols/source require debug information for the measured build".into(),
        "Target runtime version is separate from collector identity. Go reports build information from the launched executable when a Go inspection tool is available; generic native executables and subprocess toolchain versions remain unknown.".into(),
        "Linux perf permissions, Windows ETW elevation, and macOS signing restrictions are checked by samply at capture time".into(),
        "OS samples do not imply Go goroutine/scheduler/GC tracing; import an enabled runtime pprof recording for that additional route".into()] })
}

fn requested_executable(request: &CaptureRequest, name: &str) -> Option<PathBuf> {
    let supplied = Path::new(name);
    if supplied.is_absolute() || supplied.components().count() > 1 {
        let path = if supplied.is_absolute() {
            supplied.to_path_buf()
        } else {
            request.cwd.join(supplied)
        };
        return path.is_file().then(|| path.canonicalize().unwrap_or(path));
    }
    let effective_path = request
        .env
        .get("PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| env::var_os("PATH"));
    env::split_paths(&effective_path.unwrap_or_default()).find_map(|directory| {
        let directory = if directory.is_absolute() {
            directory
        } else {
            request.cwd.join(directory)
        };
        let name = if cfg!(windows) && supplied.extension().is_none() {
            format!("{name}.exe")
        } else {
            name.into()
        };
        let path = directory.join(name);
        path.is_file().then(|| path.canonicalize().unwrap_or(path))
    })
}

fn native_runtime_version(request: &CaptureRequest) -> Option<String> {
    if request.runtime.to_lowercase() != "go" {
        return None;
    }
    let target = requested_executable(request, &request.command[0])?;
    let inspector = if target
        .file_stem()?
        .to_string_lossy()
        .eq_ignore_ascii_case("go")
    {
        target.clone()
    } else {
        requested_executable(request, "go").or_else(|| tool("go"))?
    };
    let output = probe::standard(
        Command::new(inspector)
            .envs(&request.env)
            .current_dir(&request.cwd)
            .env("GOTOOLCHAIN", "local")
            .arg("version")
            .arg(target),
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    go_build_version(&String::from_utf8_lossy(&output.stdout))
}

fn go_build_version(output: &str) -> Option<String> {
    let (_, version) = output.lines().next()?.rsplit_once(": ")?;
    (version.starts_with("go") || version.starts_with("devel ")).then(|| version.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::go_build_version;

    #[test]
    fn go_runtime_build_version_does_not_include_target_output_paths() {
        assert_eq!(
            go_build_version("/tmp/control one: go1.27.3\n"),
            Some("go1.27.3".into())
        );
        assert_eq!(
            go_build_version("C:\\control one\\cpu.exe: go1.27.3\r\n"),
            Some("go1.27.3".into())
        );
        assert_eq!(
            go_build_version("/tmp/cpu: devel go1.28-abcdef\n"),
            Some("devel go1.28-abcdef".into())
        );
        assert_eq!(go_build_version("/tmp/not-go: not a Go executable\n"), None);
    }
}

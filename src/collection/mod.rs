mod bun_producers;
mod deno_workers;
mod finalization;
pub(crate) mod native_tool;
pub(crate) mod probe;
mod selection;
mod settings;
use anyhow::{Context, Result, bail};
pub use finalization::{
    CaptureArtifact, CaptureCompletion, CaptureOutput, CaptureProducer, CompletionState,
    RawCaptureBundle, RawProfile, capture_artifacts,
};
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
        CaptureAdapter::JavaScript => javascript(&canonical),
        CaptureAdapter::PythonVmprof => python(&canonical),
        CaptureAdapter::PythonCpuProgress => python_cpu(&canonical),
        CaptureAdapter::GraalCpuSampler => graal(&canonical),
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

fn walk(path: &Path, result: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_file() {
            result.push(entry.path());
        } else if kind.is_dir() {
            walk(&entry.path(), result)?;
        }
    }
    Ok(())
}

fn python(request: &CaptureRequest) -> Result<CapturePlan> {
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
    let (flags, target) = python_arguments(&request.command[1..])?;
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
    Ok(CapturePlan { command, env:BTreeMap::new(), collector_identity:None, session_identity:BTreeMap::new(),output:CaptureOutput::Profiles{format:("vmprof").into()}, runtime_version:Some(String::from_utf8_lossy(&check.stdout).trim().to_owned()),
        diagnostics:vec!["vmprof captures Python threads in one process; child processes require separate collection".into(), "CPU-clock sample times are retained without an invented wall-clock mapping".into()] })
}

fn python_cpu(request: &CaptureRequest) -> Result<CapturePlan> {
    let check=probe::standard(Command::new(&request.command[0]).envs(&request.env).current_dir(&request.cwd).args(["-c","import sys,threading; assert hasattr(threading.current_thread(),'native_id'), 'Python native thread IDs required'; print(sys.version)"])).context("inspect Python CPU sampling prerequisites")?;
    if !check.status.success() {
        bail!(
            "Python CPU progress sampling unavailable in requested interpreter: {}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
    let (flags, target) = python_arguments(&request.command[1..])?;
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
    Ok(CapturePlan{command,env:BTreeMap::new(),collector_identity:None,session_identity:BTreeMap::new(),output:CaptureOutput::Profiles{format:("python-cpu-progress").into()},runtime_version:Some(String::from_utf8_lossy(&check.stdout).trim().to_string()),diagnostics:vec!["Python stacks are gated and weighted by measured OS thread CPU progress across bounded observation windows; weights are statistical attribution, not individual call durations. Delayed observations remain unattributed CPU evidence.".into(),"GIL scheduling, native-only threads and short-lived undiscovered threads limit CPU attribution coverage; raw counters, frame snapshots and timing uncertainty are retained.".into()]})
}

fn graal(request: &CaptureRequest) -> Result<CapturePlan> {
    let (flags, target) = python_arguments(&request.command[1..])?;
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
    Ok(CapturePlan { command, env:BTreeMap::new(), collector_identity:None, session_identity:BTreeMap::new(),output:CaptureOutput::Profiles{format:("graal-cpusampler").into()}, runtime_version: probe::standard(Command::new(&request.command[0]).envs(&request.env).current_dir(&request.cwd).arg("--version")).ok().filter(|o|o.status.success()).map(|o|String::from_utf8_lossy(&o.stdout).trim().to_owned()),
        diagnostics: vec!["GraalPy distribution must expose Truffle CPUSampler; native-image and JVM-hosted modes differ".into(), "CPU sampler call-tree weights are aggregated; no per-call duration is inferred".into()] })
}

fn python_arguments(args: &[String]) -> Result<(Vec<String>, Vec<String>)> {
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

fn native(request: &CaptureRequest) -> Result<CapturePlan> {
    let samply = tool("samply").context("native/Go sampling requires Hunin's bundled samply0.13.1-hunin-native1; reinstall collector assets or set HUNIN_SAMPLY to a patched executable (an explicit override must be an existing file)")?;
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

fn javascript(request: &CaptureRequest) -> Result<CapturePlan> {
    let node = tool("node")
        .context("Node.js is required to run the JavaScript collection control adapter")?;
    let output = probe::output(
        Command::new(node).arg(assets().join("collectors/javascript/plan.mjs")),
        Some(serde_json::to_vec(request)?),
        std::time::Duration::from_secs(15),
        128 * 1024,
    )?;
    if !output.status.success() {
        bail!(
            "JavaScript capture planning failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout).context("invalid JavaScript capture plan")
}

pub(crate) fn assets() -> PathBuf {
    env::var_os("HUNIN_ASSETS_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .filter(|p| p.join("collectors").is_dir())
        })
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

pub(crate) fn vmprof_decoder() -> PathBuf {
    if let Some(value) = env::var_os("HUNIN_VMPROF_PYTHON") {
        return PathBuf::from(value);
    }
    if let Some(data) = dirs::data_local_dir() {
        let path = data.join("hunin/tools/vmprof").join(if cfg!(windows) {
            "Scripts/python.exe"
        } else {
            "bin/python"
        });
        if path.is_file() {
            return path;
        }
    }
    tool("python3")
        .or_else(|| tool("python"))
        .unwrap_or_else(|| PathBuf::from("python3"))
}

pub(crate) fn tool(name: &str) -> Option<PathBuf> {
    if let Some(path) = env::var_os(format!("HUNIN_{}", name.to_uppercase())) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
        if name == "samply" {
            return None;
        }
    }
    if name == "samply" {
        let bundled = assets()
            .join("collectors/native/bin")
            .join(if cfg!(windows) {
                "samply.exe"
            } else {
                "samply"
            });
        if bundled.is_file() {
            return Some(bundled);
        }
    }
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let path = directory.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.into()
        });
        if path.is_file() {
            return Some(path);
        }
    }
    if name == "samply" {
        let root = dirs::data_local_dir()?.join("hunin/tools");
        let mut candidates = Vec::new();
        if root.is_dir() {
            let _ = walk(&root, &mut candidates);
        }
        candidates.sort();
        return candidates.into_iter().rev().find(|p| {
            p.file_name()
                .is_some_and(|n| n == "samply" || n == "samply.exe")
        });
    }
    None
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

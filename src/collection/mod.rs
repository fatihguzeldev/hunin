pub use crate::importers::ImportedProfile;
pub(crate) mod probe;
use anyhow::{Context, Result, bail};
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
}

pub fn prepare(request: &CaptureRequest) -> Result<CapturePlan> {
    if request.command.is_empty() {
        bail!("a target command is required");
    }
    fs::create_dir_all(&request.artifact_dir)?;
    let runtime = request.runtime.to_lowercase();
    match runtime.as_str() {
        "node" | "nodejs" | "bun" | "deno" | "javascript" | "typescript" => javascript(request),
        "python" | "python3" | "cpython" | "pypy" | "pypy3" => python(request),
        "graalpy" | "graalpython" => graal(request),
        "go-test" => go_test(request, false),
        "go-trace" => go_test(request, true),
        "go" | "rust" | "c" | "cpp" | "c++" | "native" => native(request),
        other => bail!(
            "unsupported runtime {other}; choose node, bun, deno, cpython, pypy, graalpy, go, rust, c or cpp"
        ),
    }
}

pub fn collect(request: &CaptureRequest) -> Result<Vec<ImportedProfile>> {
    let mut files = Vec::new();
    walk(&request.artifact_dir, &mut files)?;
    files.sort();
    let mut profiles = Vec::new();
    for path in files {
        let format = profile_format(&path);
        if let Some(format) = format {
            profiles.push(crate::importers::import(&path, format, &request.run_id)?);
        }
    }
    if profiles.is_empty() {
        bail!(
            "collector produced no supported profile in {}; inspect target stderr and collection prerequisites",
            request.artifact_dir.display()
        );
    }
    Ok(profiles)
}

pub(crate) fn profile_format(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_string_lossy();
    if name.ends_with(".cpuprofile") {
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
    Ok(CapturePlan{command,env:BTreeMap::new(),runtime_version:probe::standard(Command::new(&request.command[0]).envs(&request.env).current_dir(&request.cwd).arg("version")).ok().filter(|o|o.status.success()).map(|r|String::from_utf8_lossy(&r.stdout).trim().to_owned()),diagnostics:vec!["Go test profiling uses one package per recording; CPU profile and execution trace are separate collection modes to avoid unnecessary profiler interference".into()]})
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
    Ok(CapturePlan { command, env:BTreeMap::new(), runtime_version:Some(String::from_utf8_lossy(&check.stdout).trim().to_owned()),
        diagnostics:vec!["vmprof captures Python threads in one process; child processes require separate collection".into(), "CPU-clock sample times are retained without an invented wall-clock mapping".into()] })
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
    Ok(CapturePlan { command, env:BTreeMap::new(), runtime_version: probe::standard(Command::new(&request.command[0]).envs(&request.env).current_dir(&request.cwd).arg("--version")).ok().filter(|o|o.status.success()).map(|o|String::from_utf8_lossy(&o.stdout).trim().to_owned()),
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
    let samply = tool("samply").context("native/Go sampling requires samply 0.13+: install the official samply release or set HUNIN_SAMPLY")?;
    let help = probe::standard(Command::new(&samply).args(["record", "--help"]))?;
    let help = String::from_utf8_lossy(&help.stdout);
    let mut command = vec![
        samply.display().to_string(),
        "record".into(),
        "--save-only".into(),
        "-o".into(),
        request
            .artifact_dir
            .join("cpu.samply.json")
            .display()
            .to_string(),
    ];
    if help.contains("--presymbolicate") {
        command.push("--presymbolicate".into());
    } else if help.contains("--unstable-presymbolicate") {
        command.push("--unstable-presymbolicate".into());
    }
    command.push("--".into());
    command.extend(request.command.clone());
    Ok(CapturePlan { command, env:BTreeMap::new(), runtime_version: version(samply.to_str().unwrap_or("samply")), diagnostics:vec![
        "Native symbols/source require debug information for the measured build".into(),
        "Linux perf permissions, Windows ETW elevation, and macOS signing restrictions are checked by samply at capture time".into(),
        "OS samples do not imply Go goroutine/scheduler/GC tracing; import an enabled runtime pprof recording for that additional route".into()] })
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

fn version(executable: &str) -> Option<String> {
    probe::standard(Command::new(executable).arg("--version"))
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

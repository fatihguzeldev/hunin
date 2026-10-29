use super::{CaptureOutput, CapturePlan, CaptureRequest};
use crate::host::{probe, tool};
use anyhow::{Result, bail};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

pub(super) fn native(request: &CaptureRequest) -> Result<CapturePlan> {
    let mut plan = super::native(request)?;
    plan.runtime_version = native_runtime_version(request);
    Ok(plan)
}

pub(super) fn test(request: &CaptureRequest, trace: bool) -> Result<CapturePlan> {
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

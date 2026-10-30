use crate::collection::{CapturePlan, CaptureRequest};
use crate::host::{probe, tool};
use anyhow::Result;
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

pub(in crate::collection) fn prepare(request: &CaptureRequest) -> Result<CapturePlan> {
    let mut plan = crate::collection::native::prepare(request)?;
    plan.runtime_version = native_runtime_version(request);
    Ok(plan)
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

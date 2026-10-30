//! Package-relative custody and the selected executable's physical identity.
use anyhow::{Context, Result, bail};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub(super) fn source_file(source: &Path, path: &str) -> Result<PathBuf> {
    relative(Path::new(path))?;
    let target = source.join(path);
    if !target.canonicalize()?.starts_with(source)
        || fs::symlink_metadata(&target)?.file_type().is_symlink()
    {
        bail!("managed analyzer package changed; register its new version before reuse: {path}");
    }
    Ok(target)
}

pub(in crate::analysis) fn executable(command: &str, cwd: &Path) -> Result<PathBuf> {
    let path = PathBuf::from(command);
    if path.is_absolute() || path.components().count() > 1 {
        return cwd
            .join(path)
            .canonicalize()
            .context("analyzer executable does not exist");
    }
    let mut names = vec![command.to_owned()];
    if cfg!(windows) && path.extension().is_none() {
        names.extend(
            std::env::var("PATHEXT")
                .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
                .split(';')
                .map(|suffix| format!("{command}{suffix}")),
        );
    }
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        for name in &names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return candidate
                    .canonicalize()
                    .context("resolve analyzer executable");
            }
        }
    }
    bail!("analyzer prerequisite missing: executable {command:?} was not found in PATH")
}

pub(super) fn relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        bail!("package files must be relative paths without parent traversal");
    }
    Ok(())
}

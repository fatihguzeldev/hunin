//! Canonical checkout roots and Git common directories identify local aliases.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

pub(super) struct Workspace {
    pub root: PathBuf,
    pub common_dir: Option<PathBuf>,
}

impl Workspace {
    pub fn discover(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .with_context(|| format!("workspace is unavailable: {}", path.display()))?;
        ensure!(path.is_dir(), "workspace must be a directory");
        for root in path.ancestors() {
            let marker = root.join(".git");
            if marker.is_dir() {
                return Ok(Self {
                    root: root.to_owned(),
                    common_dir: Some(common_dir(&marker)?),
                });
            }
            if marker.is_file() {
                let text = read_marker(&marker)?;
                let git_dir = text
                    .strip_prefix("gitdir:")
                    .context("invalid Git workspace marker")?
                    .trim();
                let git_dir = root
                    .join(git_dir)
                    .canonicalize()
                    .context("Git workspace directory is unavailable")?;
                return Ok(Self {
                    root: root.to_owned(),
                    common_dir: Some(common_dir(&git_dir)?),
                });
            }
        }
        Ok(Self {
            root: path,
            common_dir: None,
        })
    }

    pub fn default_name(&self) -> String {
        let root = self
            .common_dir
            .as_ref()
            .filter(|path| path.file_name().is_some_and(|name| name == ".git"))
            .and_then(|path| path.parent())
            .unwrap_or(&self.root);
        root.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "project".into())
    }
}

fn common_dir(git_dir: &Path) -> Result<PathBuf> {
    let marker = git_dir.join("commondir");
    if marker.is_file() {
        let text = read_marker(&marker)?;
        Ok(git_dir.join(text.trim()).canonicalize()?)
    } else {
        Ok(git_dir.canonicalize()?)
    }
}

fn read_marker(path: &Path) -> Result<String> {
    let mut text = String::new();
    fs::File::open(path)?
        .take(16 * 1024 + 1)
        .read_to_string(&mut text)?;
    ensure!(text.len() <= 16 * 1024, "Git workspace marker is too large");
    Ok(text)
}

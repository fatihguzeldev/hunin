use anyhow::Result;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn walk_files(path: &Path, result: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_file() {
            result.push(entry.path());
        } else if kind.is_dir() {
            walk_files(&entry.path(), result)?;
        }
    }
    Ok(())
}

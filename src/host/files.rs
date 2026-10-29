use anyhow::Result;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct InventoryLimits {
    pub entries: usize,
    pub files: usize,
    pub depth: usize,
}

#[derive(Default)]
pub(crate) struct Inventory {
    pub files: Vec<PathBuf>,
    pub entries: usize,
    pub regular_files: usize,
    pub nonregular_entries: usize,
    pub maximum_depth: usize,
    pub complete: bool,
    pub failure: Option<InventoryFailure>,
}

pub(crate) struct InventoryFailure {
    pub kind: &'static str,
    pub diagnostic: String,
}

pub(crate) fn inventory(
    path: &Path,
    limits: &InventoryLimits,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Inventory {
    let mut result = Inventory::default();
    match collect(path, limits, &mut result, &mut checkpoint) {
        Ok(()) => result.complete = true,
        Err(error) => result.failure = Some(error),
    }
    result.files.sort();
    result
}

fn collect(
    path: &Path,
    limits: &InventoryLimits,
    result: &mut Inventory,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> std::result::Result<(), InventoryFailure> {
    let io = |error: std::io::Error| InventoryFailure {
        kind: "io",
        diagnostic: error.to_string(),
    };
    let check = |checkpoint: &mut dyn FnMut() -> Result<()>| {
        checkpoint().map_err(|error| InventoryFailure {
            kind: "checkpoint",
            diagnostic: format!("{error:#}"),
        })
    };
    let limit = |kind| InventoryFailure {
        kind,
        diagnostic: format!("capture inventory {kind} limit exceeded"),
    };
    check(checkpoint)?;
    if !fs::symlink_metadata(path).map_err(io)?.is_dir() {
        return Err(InventoryFailure {
            kind: "io",
            diagnostic: "capture inventory root is not a directory".into(),
        });
    }
    let mut directories = vec![(fs::read_dir(path).map_err(io)?, 0usize)];
    while let Some((directory, depth)) = directories.last_mut() {
        check(checkpoint)?;
        let depth = *depth;
        let Some(entry) = directory.next() else {
            directories.pop();
            continue;
        };
        let entry = entry.map_err(io)?;
        result.entries += 1;
        if result.entries > limits.entries {
            return Err(limit("entries"));
        }
        let kind = entry.file_type().map_err(io)?;
        if kind.is_file() {
            result.regular_files += 1;
            if result.regular_files > limits.files {
                return Err(limit("files"));
            }
            result.files.push(entry.path());
        } else if kind.is_dir() {
            let depth = depth + 1;
            result.maximum_depth = result.maximum_depth.max(depth);
            if depth > limits.depth {
                return Err(limit("depth"));
            }
            check(checkpoint)?;
            directories.push((fs::read_dir(entry.path()).map_err(io)?, depth));
        } else {
            result.nonregular_entries += 1;
        }
    }
    check(checkpoint)?;
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_inventory_enforces_entry_file_and_depth_work_boundaries() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a"), b"a").unwrap();
        fs::write(root.path().join("b"), b"b").unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        fs::write(root.path().join("nested/c"), b"c").unwrap();
        let limits = InventoryLimits {
            entries: 4,
            files: 3,
            depth: 1,
        };
        let result = inventory(root.path(), &limits, || Ok(()));
        assert!(result.complete);
        assert_eq!(result.entries, 4);
        assert_eq!(result.regular_files, 3);
        assert_eq!(result.files.len(), 3);
        for (limits, reason) in [
            (
                InventoryLimits {
                    entries: 3,
                    files: 3,
                    depth: 1,
                },
                "entries",
            ),
            (
                InventoryLimits {
                    entries: 4,
                    files: 2,
                    depth: 1,
                },
                "files",
            ),
            (
                InventoryLimits {
                    entries: 4,
                    files: 3,
                    depth: 0,
                },
                "depth",
            ),
        ] {
            let result = inventory(root.path(), &limits, || Ok(()));
            assert!(!result.complete, "{reason} cap must stop traversal");
            assert_eq!(result.failure.unwrap().kind, reason);
            assert!(result.entries <= limits.entries + 1);
            assert!(result.files.len() <= limits.files);
        }
    }

    #[test]
    fn inventory_checkpoint_stops_before_entering_a_directory() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("raw"), b"bytes").unwrap();
        let result = inventory(
            root.path(),
            &InventoryLimits {
                entries: 10,
                files: 10,
                depth: 10,
            },
            || anyhow::bail!("deadline"),
        );
        assert!(!result.complete);
        assert!(result.files.is_empty());
        assert_eq!(result.entries, 0);
        assert_eq!(result.failure.unwrap().kind, "checkpoint");
    }
}

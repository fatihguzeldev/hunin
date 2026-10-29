use anyhow::Result;
use std::{
    fmt, fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(crate) struct FileReadLimit {
    pub maximum: u64,
    pub observed: u64,
}
impl fmt::Display for FileReadLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "regular-file read byte limit exceeded: maximum {}, observed {}",
            self.maximum, self.observed
        )
    }
}
impl std::error::Error for FileReadLimit {}

/// Bounded actual reads; callbacks cannot preempt an in-flight filesystem call.
pub(crate) fn visit_regular(
    path: &Path,
    maximum: u64,
    read_bytes: &mut u64,
    mut checkpoint: impl FnMut() -> Result<()>,
    mut consume: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    checkpoint()?;
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_file(),
        "input is not a regular file: {}",
        path.display()
    );
    if metadata.len() > maximum {
        return Err(FileReadLimit {
            maximum,
            observed: metadata.len(),
        }
        .into());
    }
    let mut input = fs::File::open(path)?;
    anyhow::ensure!(
        input.metadata()?.is_file(),
        "opened input is not a regular file"
    );
    let mut local = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        checkpoint()?;
        let capacity = maximum
            .saturating_sub(local)
            .saturating_add(1)
            .min(buffer.len() as u64) as usize;
        let count = input.read(&mut buffer[..capacity])?;
        *read_bytes = read_bytes
            .checked_add(count as u64)
            .ok_or_else(|| anyhow::anyhow!("file read count overflow"))?;
        local += count as u64;
        checkpoint()?;
        if local > maximum {
            return Err(FileReadLimit {
                maximum,
                observed: local,
            }
            .into());
        }
        if count == 0 {
            return Ok(());
        }
        consume(&buffer[..count])?;
        checkpoint()?;
    }
}

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
    fn regular_file_growth_after_open_reads_only_one_overflow_sentinel() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("growing");
        fs::write(&path, b"old").unwrap();
        let mut read = 0;
        let mut consumed = Vec::new();
        let mut checkpoints = 0;
        let error = visit_regular(
            &path,
            3,
            &mut read,
            || {
                checkpoints += 1;
                if checkpoints == 2 {
                    fs::write(&path, vec![7; 1000])?;
                }
                Ok(())
            },
            |chunk| {
                consumed.extend_from_slice(chunk);
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.downcast_ref::<FileReadLimit>().is_some());
        assert_eq!(read, 4);
        assert!(consumed.is_empty());
        assert_eq!(fs::metadata(path).unwrap().len(), 1000);
    }

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

use anyhow::Result;
use std::{fmt, fs, io::Read, path::Path};

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
}

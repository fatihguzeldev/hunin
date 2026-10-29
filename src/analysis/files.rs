//! Regular-file work with measured byte bounds and cooperative checkpoints.
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    io::{Read, Write},
    path::Path,
};

const CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub(super) struct ByteLimitExceeded;
impl fmt::Display for ByteLimitExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("analysis file exceeds its byte limit")
    }
}
impl std::error::Error for ByteLimitExceeded {}

fn visit(
    path: &Path,
    maximum: Option<u64>,
    mut checkpoint: impl FnMut() -> Result<()>,
    mut chunk: impl FnMut(&[u8]) -> Result<()>,
) -> Result<u64> {
    checkpoint()?;
    // External tools may have resolved aliases. Package source callers also
    // reject escaping paths/symlinks. These checks reject static special files;
    // path replacement between metadata/open is not an adversarial FS sandbox.
    let metadata =
        fs::metadata(path).with_context(|| format!("read analysis file {}", path.display()))?;
    ensure!(
        metadata.is_file(),
        "analysis input is not a regular file: {}",
        path.display()
    );
    if maximum.is_some_and(|maximum| metadata.len() > maximum) {
        return Err(ByteLimitExceeded.into());
    }
    checkpoint()?;
    let mut input = fs::File::open(path)?;
    ensure!(
        input.metadata()?.is_file(),
        "opened analysis input is not a regular file: {}",
        path.display()
    );
    let mut bytes = 0u64;
    let mut buffer = [0u8; CHUNK_BYTES];
    loop {
        checkpoint()?;
        // At the boundary, read one extra byte to distinguish EOF from growth.
        let capacity = maximum
            .map(|maximum| {
                maximum
                    .saturating_sub(bytes)
                    .saturating_add(1)
                    .min(CHUNK_BYTES as u64) as usize
            })
            .unwrap_or(CHUNK_BYTES);
        let count = input.read(&mut buffer[..capacity])?;
        checkpoint()?;
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(count as u64)
            .context("analysis file size overflow")?;
        if maximum.is_some_and(|maximum| bytes > maximum) {
            return Err(ByteLimitExceeded.into());
        }
        chunk(&buffer[..count])?;
    }
    checkpoint()?;
    Ok(bytes)
}

pub(super) fn read(
    path: &Path,
    maximum: u64,
    checkpoint: impl FnMut() -> Result<()>,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    visit(path, Some(maximum), checkpoint, |chunk| {
        bytes.try_reserve(chunk.len())?;
        bytes.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok(bytes)
}

fn identify(
    path: &Path,
    maximum: Option<u64>,
    checkpoint: impl FnMut() -> Result<()>,
) -> Result<(String, u64)> {
    let mut hash = Sha256::new();
    let bytes = visit(path, maximum, checkpoint, |chunk| {
        hash.update(chunk);
        Ok(())
    })?;
    Ok((format!("{:x}", hash.finalize()), bytes))
}

pub(super) fn hash(path: &Path, checkpoint: impl FnMut() -> Result<()>) -> Result<String> {
    Ok(identify(path, None, checkpoint)?.0)
}

pub(super) fn hash_bounded(
    path: &Path,
    maximum: u64,
    checkpoint: impl FnMut() -> Result<()>,
) -> Result<(String, u64)> {
    identify(path, Some(maximum), checkpoint)
}

pub(super) fn copy(
    source: &Path,
    destination: &Path,
    maximum: u64,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<(String, u64)> {
    checkpoint()?;
    let mut output = fs::File::create(destination)?;
    let mut hash = Sha256::new();
    let bytes = visit(source, Some(maximum), checkpoint, |chunk| {
        hash.update(chunk);
        output.write_all(chunk)?;
        Ok(())
    })?;
    output.flush()?;
    Ok((format!("{:x}", hash.finalize()), bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_reader_rejects_bytes_above_its_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input");
        fs::write(&path, b"larger than allowed").unwrap();
        assert!(read(&path, 8, || Ok(())).is_err());
    }

    #[test]
    fn chunked_hash_copy_and_read_keep_exact_existing_bytes_and_sha() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("copy");
        let bytes: Vec<_> = (0..(CHUNK_BYTES * 3 + 19))
            .map(|index| (index % 251) as u8)
            .collect();
        fs::write(&source, &bytes).unwrap();
        let expected = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(hash(&source, || Ok(())).unwrap(), expected);
        assert_eq!(read(&source, bytes.len() as u64, || Ok(())).unwrap(), bytes);
        assert_eq!(
            copy(&source, &destination, bytes.len() as u64, || Ok(())).unwrap(),
            (expected, bytes.len() as u64)
        );
        assert_eq!(fs::read(destination).unwrap(), bytes);
    }

    #[test]
    fn cancellation_can_stop_before_the_next_read_chunk() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        fs::write(&source, vec![0; CHUNK_BYTES * 3]).unwrap();
        let (sender, cancellation) = tokio::sync::watch::channel(false);
        let control = crate::analysis::process::Control::new(
            cancellation,
            tokio::time::Instant::now() + std::time::Duration::from_secs(30),
        );
        let mut visited = 0;
        let error = visit(
            &source,
            None,
            || control.check(),
            |chunk| {
                visited += chunk.len();
                sender.send(true).unwrap();
                Ok(())
            },
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("analysis cancelled"));
        assert_eq!(visited, CHUNK_BYTES);
    }

    #[test]
    fn growth_after_metadata_is_checked_against_actual_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        fs::write(&source, vec![1; CHUNK_BYTES]).unwrap();
        let mut visited = 0;
        let error = visit(
            &source,
            Some(CHUNK_BYTES as u64),
            || Ok(()),
            |chunk| {
                visited += chunk.len();
                fs::OpenOptions::new()
                    .append(true)
                    .open(&source)?
                    .write_all(b"growth")?;
                Ok(())
            },
        )
        .err()
        .unwrap();
        assert!(error.downcast_ref::<ByteLimitExceeded>().is_some());
        assert_eq!(visited, CHUNK_BYTES);
    }

    #[cfg(unix)]
    #[test]
    fn a_static_fifo_is_rejected_before_its_blocking_open() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("pipe");
        let name = std::ffi::CString::new(source.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (done, completion) = std::sync::mpsc::channel();
        let fallback = source.clone();
        let thread = std::thread::spawn(move || {
            if completion
                .recv_timeout(std::time::Duration::from_secs(2))
                .is_err()
            {
                drop(fs::File::create(fallback).unwrap());
            }
        });
        let result = read(&source, 1024, || Ok(()));
        let _ = done.send(());
        thread.join().unwrap();
        let error = result.unwrap_err();
        assert!(error.to_string().contains("not a regular file"));
    }
}

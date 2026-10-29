//! Stable, bounded byte inputs shared by format readers and external decoders.
use crate::host::visit_regular;
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(crate) struct NormalizationByteLimit {
    pub maximum: u64,
    pub observed: u64,
}
impl fmt::Display for NormalizationByteLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "profile-normalization-input-v1 decoded bytes exceed limit {} (observed {})",
            self.maximum, self.observed
        )
    }
}
impl std::error::Error for NormalizationByteLimit {}

#[derive(Clone, Copy, Serialize)]
pub(crate) struct Limits {
    pub raw_bytes: u64,
    pub sidecar_bytes: u64,
    pub decompressed_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            raw_bytes: 512 * 1024 * 1024,
            sidecar_bytes: 256 * 1024 * 1024,
            decompressed_bytes: 512 * 1024 * 1024,
        }
    }
}

struct File {
    path: PathBuf,
    bytes: u64,
    sha256: String,
}

pub(super) struct Snapshot {
    directory: PathBuf,
    raw: Option<File>,
    sidecars: BTreeMap<PathBuf, File>,
    sidecar_reads: u64,
    limits: Limits,
    deadline: Option<Instant>,
}

impl Snapshot {
    pub fn new(limits: Limits, deadline: Option<Instant>) -> Result<Self> {
        check(deadline)?;
        let directory =
            std::env::temp_dir().join(format!("hunin-normalization-{}", uuid::Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        Ok(Self {
            directory,
            raw: None,
            sidecars: BTreeMap::new(),
            sidecar_reads: 0,
            limits,
            deadline,
        })
    }
    pub fn capture_raw(&mut self, path: &Path, digest: &mut Sha256) -> Result<()> {
        self.raw = Some(self.copy(path, self.limits.raw_bytes, &mut 0, digest)?);
        Ok(())
    }
    pub fn capture_sidecar(
        &mut self,
        path: &Path,
        maximum: u64,
        digest: &mut Sha256,
    ) -> Result<()> {
        let maximum = maximum.min(self.limits.sidecar_bytes.saturating_sub(self.sidecar_reads));
        let mut read = 0;
        let file = self.copy(path, maximum, &mut read, digest);
        self.sidecar_reads += read;
        self.sidecars.insert(path.to_owned(), file?);
        Ok(())
    }
    fn copy(
        &self,
        source: &Path,
        maximum: u64,
        read_bytes: &mut u64,
        digest: &mut Sha256,
    ) -> Result<File> {
        let path = self.directory.join(
            source
                .file_name()
                .context("normalization input filename missing")?,
        );
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = options.open(&path)?;
        let mut hash = Sha256::new();
        visit_regular(
            source,
            maximum,
            read_bytes,
            || check(self.deadline),
            |chunk| {
                hash.update(chunk);
                digest.update(chunk);
                output.write_all(chunk)?;
                Ok(())
            },
        )
        .with_context(|| {
            format!(
                "profile-normalization-input-v1 capture {}",
                source.display()
            )
        })?;
        output.flush()?;
        check(self.deadline)?;
        Ok(File {
            path,
            bytes: *read_bytes,
            sha256: format!("{:x}", hash.finalize()),
        })
    }
    pub fn path(&self) -> &Path {
        &self.raw.as_ref().expect("raw input captured").path
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
    pub fn remaining(&self, maximum: Duration) -> Result<Duration> {
        check(self.deadline)?;
        Ok(self
            .deadline
            .map(|deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(maximum)
            })
            .unwrap_or(maximum))
    }
    pub fn raw_sha256(&self) -> &str {
        &self.raw.as_ref().expect("raw input captured").sha256
    }
    pub fn sidecar_sha256(&self) -> BTreeMap<PathBuf, String> {
        self.sidecars
            .iter()
            .map(|(path, file)| (path.clone(), file.sha256.clone()))
            .collect()
    }
    pub fn decoded(&self) -> Result<Vec<u8>> {
        let mut input = BufReader::new(fs::File::open(self.path())?);
        let gzip = input.fill_buf()?.starts_with(&[0x1f, 0x8b]);
        if gzip {
            decode(
                &mut flate2::read::GzDecoder::new(input),
                self.limits.decompressed_bytes,
                self.deadline,
            )
        } else {
            decode(&mut input, self.limits.raw_bytes, self.deadline)
        }
    }
    pub fn verify(&self) -> Result<()> {
        for file in self.raw.iter().chain(self.sidecars.values()) {
            let mut hash = Sha256::new();
            visit_regular(
                &file.path,
                file.bytes,
                &mut 0,
                || check(self.deadline),
                |chunk| {
                    hash.update(chunk);
                    Ok(())
                },
            )?;
            ensure!(
                format!("{:x}", hash.finalize()) == file.sha256,
                "normalization input snapshot changed during decode"
            );
        }
        Ok(())
    }
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

pub(super) fn check(deadline: Option<Instant>) -> Result<()> {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        bail!("profile-normalization-input-v1 checkpoint deadline elapsed");
    }
    Ok(())
}

fn decode(input: &mut impl Read, maximum: u64, deadline: Option<Instant>) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check(deadline)?;
        let capacity = maximum
            .saturating_sub(decoded.len() as u64)
            .saturating_add(1)
            .min(buffer.len() as u64) as usize;
        let count = input.read(&mut buffer[..capacity])?;
        check(deadline)?;
        let observed = decoded.len() as u64 + count as u64;
        if observed > maximum {
            return Err(NormalizationByteLimit { maximum, observed }.into());
        }
        if count == 0 {
            return Ok(decoded);
        }
        decoded.try_reserve(count)?;
        decoded.extend_from_slice(&buffer[..count]);
    }
}

#[cfg(test)]
mod tests;

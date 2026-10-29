use super::Store;
use crate::model::runs::Artifact;
use anyhow::{Context, Result, bail};
use rusqlite::{TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(crate) struct ArtifactByteLimit {
    pub maximum: u64,
    pub observed: u64,
}
impl fmt::Display for ArtifactByteLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "artifact source exceeds its byte limit of {} (observed {})",
            self.maximum, self.observed
        )
    }
}
impl std::error::Error for ArtifactByteLimit {}

impl Store {
    pub(crate) fn add_artifact_controlled(
        &self,
        run_id: &str,
        source: &Path,
        format: &str,
        maximum_bytes: u64,
        read_bytes: &mut u64,
        mut checkpoint: impl FnMut() -> Result<()>,
    ) -> Result<Artifact> {
        checkpoint()?;
        let name = source
            .file_name()
            .context("artifact filename missing")?
            .to_string_lossy();
        validate_artifact_name(&name)?;
        self.run(run_id)?;
        let metadata = fs::symlink_metadata(source)?;
        anyhow::ensure!(
            metadata.is_file(),
            "artifact source is not a regular file: {}",
            source.display()
        );
        if metadata.len() > maximum_bytes {
            return Err(ArtifactByteLimit {
                maximum: maximum_bytes,
                observed: metadata.len(),
            }
            .into());
        }
        checkpoint()?;
        let mut input = fs::File::open(source)
            .with_context(|| format!("read artifact {}", source.display()))?;
        anyhow::ensure!(
            input.metadata()?.is_file(),
            "opened artifact source is not a regular file"
        );
        let pending = self.stage_artifact(
            &mut input,
            format,
            Some(maximum_bytes),
            read_bytes,
            &mut checkpoint,
        )?;
        self.publish_artifact(run_id, &name, format, pending, &mut checkpoint)
    }
    pub fn add_artifact(&self, run_id: &str, source: &Path, format: &str) -> Result<Artifact> {
        let name = source
            .file_name()
            .context("artifact filename missing")?
            .to_string_lossy();
        validate_artifact_name(&name)?;
        self.run(run_id)?;
        let mut input = fs::File::open(source)
            .with_context(|| format!("read artifact {}", source.display()))?;
        let pending = self.stage_artifact(&mut input, format, None, &mut 0, &mut || Ok(()))?;
        self.publish_artifact(run_id, &name, format, pending, &mut || Ok(()))
    }

    pub fn add_artifact_bytes(
        &self,
        run_id: &str,
        name: &str,
        format: &str,
        bytes: &[u8],
    ) -> Result<Artifact> {
        validate_artifact_name(name)?;
        self.run(run_id)?;
        let pending = self.stage_artifact(
            &mut std::io::Cursor::new(bytes),
            format,
            None,
            &mut 0,
            &mut || Ok(()),
        )?;
        self.publish_artifact(run_id, name, format, pending, &mut || Ok(()))
    }

    fn stage_artifact(
        &self,
        input: &mut impl Read,
        format: &str,
        maximum_bytes: Option<u64>,
        read_bytes: &mut u64,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<PendingArtifact> {
        checkpoint()?;
        let path = self
            .root
            .join("artifacts")
            .join(format!(".pending-{}", uuid::Uuid::new_v4()));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut pending = PendingArtifact {
            path,
            file: Some(file),
            sha256: String::new(),
            size: 0,
        };
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            checkpoint()?;
            let capacity = maximum_bytes
                .map(|maximum| {
                    maximum
                        .saturating_sub(pending.size)
                        .saturating_add(1)
                        .min(buffer.len() as u64) as usize
                })
                .unwrap_or(buffer.len());
            let count = input
                .read(&mut buffer[..capacity])
                .context("read artifact bytes")?;
            *read_bytes = read_bytes
                .checked_add(count as u64)
                .context("artifact read count overflow")?;
            checkpoint()?;
            if count == 0 {
                break;
            }
            pending.size = pending
                .size
                .checked_add(count as u64)
                .context("artifact size overflow")?;
            if let Some(maximum) = maximum_bytes.filter(|maximum| pending.size > *maximum) {
                return Err(ArtifactByteLimit {
                    maximum,
                    observed: pending.size,
                }
                .into());
            }
            anyhow::ensure!(
                format != "resolved-source" || pending.size <= 2 * 1024 * 1024,
                "resolved source exceeds the 2 MiB file limit"
            );
            hash.update(&buffer[..count]);
            pending
                .file
                .as_mut()
                .expect("staged artifact is open")
                .write_all(&buffer[..count])
                .context("write artifact bytes")?;
            checkpoint()?;
        }
        checkpoint()?;
        pending.file.as_ref().unwrap().sync_all()?;
        checkpoint()?;
        // Close before rename or cleanup, including on Windows.
        pending.file.take();
        pending.sha256 = format!("{:x}", hash.finalize());
        Ok(pending)
    }

    fn publish_artifact(
        &self,
        run_id: &str,
        name: &str,
        format: &str,
        pending: PendingArtifact,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Artifact> {
        checkpoint()?;
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        checkpoint()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1)",
            [run_id],
            |r| r.get(0),
        )?;
        anyhow::ensure!(exists, "run was removed before artifact publication");
        if format == "resolved-source" {
            let (retained, duplicate): (u64, bool) = tx.query_row(
                "SELECT COALESCE(SUM(size),0),COALESCE(MAX(sha256=?2),0) FROM (SELECT sha256,MAX(CAST(json_extract(metadata,'$.size') AS INTEGER)) AS size FROM artifacts WHERE run_id=?1 AND json_extract(metadata,'$.format')='resolved-source' GROUP BY sha256)",
                params![run_id, pending.sha256],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            anyhow::ensure!(
                duplicate || retained.saturating_add(pending.size) <= 32 * 1024 * 1024,
                "resolved source snapshots exceed the 32 MiB recording limit"
            );
        }
        let dest = self.root.join("artifacts").join(&pending.sha256);
        // Stage large inputs before taking the run/GC write lock. Publish only
        // complete bytes while that lock excludes deletion and other writers.
        if !dest.exists() {
            fs::rename(&pending.path, &dest)?;
            // The file is already synced. Also persist the directory entry on
            // Unix filesystems that support syncing directories.
            #[cfg(unix)]
            if let Ok(directory) = fs::File::open(self.root.join("artifacts")) {
                let _ = directory.sync_all();
            }
        } else if !same_artifact_contents(&pending.path, &dest, checkpoint)? {
            bail!("artifact identity mismatch");
        }
        let reference_id = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(&pending.sha256, &name, format))?)
        );
        let artifact = Artifact {
            id: reference_id,
            name: name.to_owned(),
            size: pending.size,
            sha256: pending.sha256.clone(),
            format: format.into(),
        };
        checkpoint()?;
        tx.execute(
            "INSERT OR IGNORE INTO artifacts(run_id,id,metadata,sha256) VALUES(?1,?2,?3,?4)",
            params![
                run_id,
                artifact.id,
                serde_json::to_string(&artifact)?,
                artifact.sha256
            ],
        )?;
        checkpoint()?;
        tx.commit()?;
        Ok(artifact)
    }

    pub fn artifacts(&self, run_id: &str) -> Result<Vec<Artifact>> {
        self.run(run_id)?;
        let c = self.connection()?;
        let mut statement =
            c.prepare("SELECT metadata FROM artifacts WHERE run_id=?1 ORDER BY id")?;
        statement
            .query_map([run_id], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }

    pub fn artifacts_page(
        &self,
        run_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Artifact>> {
        self.run(run_id)?;
        let c = self.connection()?;
        let mut statement = c.prepare(
            "SELECT metadata FROM artifacts WHERE run_id=?1 ORDER BY id LIMIT ?2 OFFSET ?3",
        )?;
        statement
            .query_map(params![run_id, limit.min(501), offset], |r| {
                r.get::<_, String>(0)
            })?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }

    pub fn artifact_path(&self, run_id: &str, id: &str) -> Result<PathBuf> {
        let artifact = self.artifact_metadata(run_id, id)?;
        Ok(self.root.join("artifacts").join(artifact.sha256))
    }

    pub(crate) fn artifact_metadata(&self, run_id: &str, id: &str) -> Result<Artifact> {
        // Resolve through a run-owned reference, never by joining an arbitrary caller path.
        self.run(run_id)?;
        let (metadata, indexed_sha): (String, String) = self
            .connection()?
            .query_row(
                "SELECT metadata,sha256 FROM artifacts WHERE run_id=?1 AND id=?2",
                params![run_id, id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .context("artifact not found")?;
        let artifact: Artifact = serde_json::from_str(&metadata)?;
        anyhow::ensure!(
            super::maintenance::is_artifact_digest(&artifact.sha256),
            "retained artifact digest is not a canonical SHA-256"
        );
        anyhow::ensure!(
            artifact.sha256 == indexed_sha && artifact.id == id,
            "artifact metadata disagrees with its indexed identity"
        );
        Ok(artifact)
    }
}

struct PendingArtifact {
    path: PathBuf,
    file: Option<fs::File>,
    sha256: String,
    size: u64,
}

impl Drop for PendingArtifact {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

fn validate_artifact_name(name: &str) -> Result<()> {
    anyhow::ensure!(
        !name.is_empty() && !matches!(name, "." | "..") && !name.contains(['/', '\\', '\0']),
        "artifact name must be a filename"
    );
    Ok(())
}

fn same_artifact_contents(
    first: &Path,
    second: &Path,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<bool> {
    checkpoint()?;
    let mut first = fs::File::open(first)?;
    let mut second = fs::File::open(second)?;
    let size = first.metadata()?.len();
    if size != second.metadata()?.len() {
        return Ok(false);
    }
    let mut remaining = size;
    let mut a = [0u8; 64 * 1024];
    let mut b = [0u8; 64 * 1024];
    while remaining > 0 {
        checkpoint()?;
        let count = remaining.min(a.len() as u64) as usize;
        first.read_exact(&mut a[..count])?;
        checkpoint()?;
        second.read_exact(&mut b[..count])?;
        checkpoint()?;
        if a[..count] != b[..count] {
            return Ok(false);
        }
        remaining -= count as u64;
    }
    Ok(true)
}

#[cfg(test)]
mod tests;

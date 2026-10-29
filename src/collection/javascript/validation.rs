//! Owned validation reads; limits are work policy, not producer loss coverage.
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::fs;
use std::{path::Path, time::Instant};

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub profile_bytes: u64,
    pub metadata_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            profile_bytes: 512 * 1024 * 1024,
            metadata_bytes: 8 * 1024 * 1024,
        }
    }
}

pub(in crate::collection) struct Reads {
    limits: Limits,
    deadline: Option<Instant>,
    metadata_bytes: u64,
    raw_bytes: u64,
}
impl Reads {
    pub fn new(deadline: Option<Instant>) -> Self {
        Self {
            limits: Limits::default(),
            deadline,
            metadata_bytes: 0,
            raw_bytes: 0,
        }
    }
    #[cfg(test)]
    pub(super) fn with_limits(limits: Limits, deadline: Option<Instant>) -> Self {
        Self {
            limits,
            ..Self::new(deadline)
        }
    }
    pub fn check(&self) -> Result<()> {
        checkpoint(self.deadline)
    }
    pub fn metadata(&mut self, path: &Path) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let maximum = self
            .limits
            .metadata_bytes
            .saturating_sub(self.metadata_bytes);
        let deadline = self.deadline;
        crate::host::visit_regular(
            path,
            maximum,
            &mut self.metadata_bytes,
            || checkpoint(deadline),
            |chunk| {
                bytes.try_reserve(chunk.len())?;
                bytes.extend_from_slice(chunk);
                Ok(())
            },
        )
        .with_context(|| {
            format!(
                "collector-validation-read-v1 metadata {} ({} byte aggregate limit)",
                path.display(),
                self.limits.metadata_bytes
            )
        })?;
        Ok(bytes)
    }
    pub fn raw_digest(&mut self, path: &Path) -> Result<String> {
        digest(
            path,
            self.limits.profile_bytes,
            &mut self.raw_bytes,
            self.deadline,
        )
        .with_context(|| {
            format!(
                "collector-validation-read-v1 raw profile {}",
                path.display()
            )
        })
    }
    pub fn metadata_digest(&mut self, path: &Path) -> Result<String> {
        digest(
            path,
            self.limits
                .metadata_bytes
                .saturating_sub(self.metadata_bytes),
            &mut self.metadata_bytes,
            self.deadline,
        )
        .with_context(|| {
            format!(
                "collector-validation-read-v1 clock metadata {} ({} byte aggregate limit)",
                path.display(),
                self.limits.metadata_bytes
            )
        })
    }
}

fn checkpoint(deadline: Option<Instant>) -> Result<()> {
    ensure!(
        deadline.is_none_or(|deadline| Instant::now() < deadline),
        "collector validation checkpoint deadline elapsed"
    );
    Ok(())
}
fn digest(
    path: &Path,
    maximum: u64,
    read_bytes: &mut u64,
    deadline: Option<Instant>,
) -> Result<String> {
    let mut hash = Sha256::new();
    crate::host::visit_regular(
        path,
        maximum,
        read_bytes,
        || checkpoint(deadline),
        |chunk| {
            hash.update(chunk);
            Ok(())
        },
    )?;
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_over_metadata_reads_share_one_budget_and_leave_files_untouched() {
        let root = tempfile::tempdir().unwrap();
        for name in ["one.json", "two.json", "three.json"] {
            fs::write(root.path().join(name), b"{}").unwrap();
        }
        let mut reads = Reads::new(None);
        reads.limits.metadata_bytes = 4;
        assert_eq!(
            reads.metadata(&root.path().join("one.json")).unwrap(),
            b"{}"
        );
        assert_eq!(
            reads
                .metadata_digest(&root.path().join("two.json"))
                .unwrap(),
            format!("{:x}", Sha256::digest(b"{}"))
        );
        assert_eq!(reads.metadata_bytes, 4);
        assert!(reads.metadata(&root.path().join("three.json")).is_err());
        assert_eq!(fs::read(root.path().join("three.json")).unwrap(), b"{}");
    }

    #[test]
    fn streamed_profile_hash_is_bounded_and_an_expired_deadline_reads_no_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("profile");
        fs::write(&path, b"raw").unwrap();
        let mut reads = Reads::new(None);
        reads.limits.profile_bytes = 3;
        assert_eq!(
            reads.raw_digest(&path).unwrap(),
            format!("{:x}", Sha256::digest(b"raw"))
        );
        assert_eq!(reads.raw_bytes, 3);
        fs::write(&path, b"grown").unwrap();
        assert!(reads.raw_digest(&path).is_err());
        let mut expired = Reads::new(Some(Instant::now()));
        assert!(expired.raw_digest(&path).is_err());
        assert!(expired.metadata(&path).is_err());
        assert_eq!(expired.raw_bytes + expired.metadata_bytes, 0);
        assert_eq!(fs::read(path).unwrap(), b"grown");
    }
}

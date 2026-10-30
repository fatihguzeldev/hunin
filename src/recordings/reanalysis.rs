//! Restore retained inputs under bounded work, normalize again and republish.
use super::{Publication, checkpoint, publication::publish_with_limits};
use crate::{model::runs::Artifact, storage::Store};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::{fmt, fs, io::Write, path::Path, time::Instant};

#[derive(Debug)]
pub(crate) struct ReanalysisSidecarLimit {
    pub maximum: usize,
    pub observed: usize,
}
impl fmt::Display for ReanalysisSidecarLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "profile-normalization-input-v1 restored sidecars exceed {} file limit (observed {} manifest entries)",
            self.maximum, self.observed
        )
    }
}
impl std::error::Error for ReanalysisSidecarLimit {}

pub(crate) fn reanalyze_controlled(
    store: &Store,
    run_id: &str,
    artifact_id: &str,
    deadline: Option<std::time::Instant>,
) -> Result<Publication> {
    reanalyze_with_limits(
        store,
        run_id,
        artifact_id,
        crate::importers::NormalizationInputLimits::default(),
        deadline,
    )
}

pub(super) fn reanalyze_with_limits(
    store: &Store,
    run_id: &str,
    artifact_id: &str,
    limits: crate::importers::NormalizationInputLimits,
    deadline: Option<Instant>,
) -> Result<Publication> {
    checkpoint(deadline)?;
    let mut components = Path::new(run_id).components();
    anyhow::ensure!(
        matches!(components.next(), Some(std::path::Component::Normal(_)))
            && components.next().is_none(),
        "recording ID cannot name a managed directory"
    );
    let selected = store.artifact_metadata(run_id, artifact_id)?;
    let inputs = store.document("profile-inputs",&format!("{run_id}:{}",selected.sha256)).context("recording lacks a retained import manifest; import the original recording and its sidecars first")?;
    let raw: Artifact = serde_json::from_value(inputs["raw"].clone())?;
    anyhow::ensure!(
        raw.sha256 == selected.sha256 && inputs["run_id"] == run_id,
        "retained import manifest does not match requested recording/raw identity"
    );
    let directory = store
        .root()
        .join("captures")
        .join(run_id)
        .join("reanalysis")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(directory.parent().context("reanalysis parent missing")?)?;
    fs::create_dir(&directory)?;
    struct RestoreDirectory(std::path::PathBuf);
    impl Drop for RestoreDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = RestoreDirectory(directory.clone());
    let path = restore(
        store,
        run_id,
        &raw,
        &directory,
        limits.raw_bytes,
        &mut 0,
        deadline,
    )?;
    let sidecars = inputs["sidecars"]
        .as_array()
        .context("retained sidecar manifest missing")?;
    if sidecars.len() > 1024 {
        return Err(ReanalysisSidecarLimit {
            maximum: 1024,
            observed: sidecars.len(),
        }
        .into());
    }
    let mut names = std::collections::BTreeMap::from([(raw.name.clone(), raw.sha256.clone())]);
    let mut restored_sidecar_bytes = 0;
    for sidecar in sidecars {
        let sidecar: Artifact = serde_json::from_value(sidecar.clone())?;
        if let Some(sha) = names.get(&sidecar.name) {
            anyhow::ensure!(
                sha == &sidecar.sha256,
                "retained input filenames disagree on content identity"
            );
            continue;
        }
        restore(
            store,
            run_id,
            &sidecar,
            &directory,
            limits.sidecar_bytes.saturating_sub(restored_sidecar_bytes),
            &mut restored_sidecar_bytes,
            deadline,
        )?;
        names.insert(sidecar.name, sidecar.sha256);
    }
    let mut profile = if let Some(id) = inputs["native_context_artifact_id"].as_str() {
        crate::importers::import_with_native_context_controlled(
            &path,
            &raw.format,
            run_id,
            &store.artifact_path(run_id, id)?,
            deadline,
        )?
    } else {
        crate::importers::import_controlled(&path, &raw.format, run_id, deadline)?
    };
    if profile.decoder_provenance.is_some() && inputs["decoder_identity"].is_null() {
        profile.diagnostics.push("Legacy normalization has no decoder identity; this reanalysis uses the current identified decoder and cannot establish the historical decoder environment.".into());
    }
    publish_with_limits(store, run_id, profile, limits, deadline)
}

fn restore(
    store: &Store,
    run_id: &str,
    artifact: &Artifact,
    directory: &Path,
    maximum: u64,
    read_bytes: &mut u64,
    deadline: Option<Instant>,
) -> Result<std::path::PathBuf> {
    anyhow::ensure!(
        !artifact.name.is_empty()
            && !artifact.name.contains(['/', '\\', '\0'])
            && !matches!(artifact.name.as_str(), "." | ".."),
        "invalid retained artifact filename"
    );
    let path = directory.join(&artifact.name);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(&path)?;
    let mut hash = Sha256::new();
    crate::host::visit_regular(
        &store.artifact_path(run_id, &artifact.id)?,
        maximum,
        read_bytes,
        || checkpoint(deadline),
        |chunk| {
            hash.update(chunk);
            output.write_all(chunk)?;
            Ok(())
        },
    )
    .context("profile-normalization-input-v1 retained input restore")?;
    anyhow::ensure!(
        format!("{:x}", hash.finalize()) == artifact.sha256,
        "retained profile/sidecar integrity mismatch"
    );
    output.flush()?;
    checkpoint(deadline)?;
    Ok(path)
}

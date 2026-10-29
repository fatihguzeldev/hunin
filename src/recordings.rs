use crate::publication::{PublicationMetadata, PublicationRequest};
use crate::{importers::ImportedProfile, model::runs::Artifact, storage::Store};
use anyhow::{Context, Result};
use serde_json::{Value, json};
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

pub(crate) struct Publication {
    pub accepted: bool,
    pub diagnostics: Vec<String>,
}

#[cfg(test)]
pub(crate) fn publish(
    store: &Store,
    run_id: &str,
    profile: ImportedProfile,
) -> Result<Publication> {
    publish_controlled(store, run_id, profile, None)
}

pub(crate) fn publish_controlled(
    store: &Store,
    run_id: &str,
    profile: ImportedProfile,
    deadline: Option<std::time::Instant>,
) -> Result<Publication> {
    publish_with_limits(
        store,
        run_id,
        profile,
        crate::importers::NormalizationInputLimits::default(),
        deadline,
    )
}

fn publish_with_limits(
    store: &Store,
    run_id: &str,
    profile: ImportedProfile,
    limits: crate::importers::NormalizationInputLimits,
    deadline: Option<Instant>,
) -> Result<Publication> {
    let raw = store.add_artifact_controlled(
        run_id,
        &profile.source,
        &profile.format,
        limits.raw_bytes,
        &mut 0,
        || checkpoint(deadline),
    )?;
    anyhow::ensure!(
        raw.sha256 == profile.raw_sha256,
        "raw recording changed during normalization; retry with stable inputs"
    );
    let mut sidecars = Vec::new();
    let mut sidecar_reads = 0;
    for path in &profile.sidecars {
        let artifact = store.add_artifact_controlled(
            run_id,
            path,
            "profile-sidecar",
            limits.sidecar_bytes.saturating_sub(sidecar_reads),
            &mut sidecar_reads,
            || checkpoint(deadline),
        )?;
        anyhow::ensure!(
            profile.sidecar_sha256.get(path) == Some(&artifact.sha256),
            "profile sidecar changed during normalization; retry with stable inputs"
        );
        sidecars.push(artifact);
    }
    for image in &profile.binary_snapshots {
        checkpoint(deadline)?;
        anyhow::ensure!(
            format!("{:x}", Sha256::digest(&image.content)) == image.sha256,
            "native image snapshot digest mismatch"
        );
        store.add_artifact_bytes(
            run_id,
            &format!("image-{}.bin", image.sha256),
            "native-image",
            &image.content,
        )?;
        checkpoint(deadline)?;
    }
    let context = profile
        .native_image_context
        .as_ref()
        .map(|value| {
            checkpoint(deadline)?;
            store.add_artifact_bytes(
                run_id,
                "native-images.json",
                "native-image-context",
                &serde_json::to_vec(value)?,
            )
        })
        .transpose()?;
    let mut sources = Vec::new();
    for source in &profile.source_snapshots {
        checkpoint(deadline)?;
        anyhow::ensure!(
            format!("{:x}", Sha256::digest(source.content.as_bytes())) == source.sha256,
            "resolved source snapshot digest mismatch"
        );
        let artifact = store.add_artifact_bytes(
            run_id,
            &format!("source-{}.txt", source.sha256),
            "resolved-source",
            source.content.as_bytes(),
        )?;
        sources.push(json!({"uri":source.uri,"artifact_id":artifact.id,"sha256":source.sha256,"provenance":source.provenance}));
        checkpoint(deadline)?;
    }
    let id = format!("{run_id}:{}", raw.sha256);
    let decoder = profile
        .decoder_provenance
        .as_ref()
        .map(|value| {
            checkpoint(deadline)?;
            store.add_artifact_bytes(
                run_id,
                "decoder-provenance.json",
                "decoder-provenance",
                &serde_json::to_vec(&json!({
                    "normalization_fingerprint":profile.input_fingerprint,
                    "raw_sha256":profile.raw_sha256,
                    "decoder":value,
                }))?,
            )
        })
        .transpose()?;
    let document = json!({
        "run_id":run_id,
        "raw":raw,
        "sidecars":sidecars,
        "native_context_artifact_id":context.map(|artifact|artifact.id),
        "source_snapshots":sources,
        "normalization_fingerprint":profile.input_fingerprint,
        "decoder_provenance_artifact_id":decoder.map(|artifact|artifact.id),
        "decoder_identity":profile.decoder_provenance.as_ref().map(|value|&value["identity"]),
        "status":"accepted",
    });
    let accepted = store.publish_result(PublicationRequest {
        run_id,
        source_key: &format!("import-v1:{}", raw.sha256),
        input_identity: &format!("normalization:{}", profile.input_fingerprint),
        records: &profile.records,
        metadata: Some(PublicationMetadata::Normalization {
            input_id: &id,
            document: &document,
        }),
        deadline,
    })?;
    Ok(Publication {
        accepted,
        diagnostics: profile.diagnostics,
    })
}

#[cfg(test)]
pub(crate) fn reanalyze(store: &Store, run_id: &str, artifact_id: &str) -> Result<Publication> {
    reanalyze_controlled(store, run_id, artifact_id, None)
}

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

fn reanalyze_with_limits(
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

fn checkpoint(deadline: Option<Instant>) -> Result<()> {
    anyhow::ensure!(
        deadline.is_none_or(|deadline| Instant::now() < deadline),
        "normalization finishing checkpoint deadline elapsed"
    );
    Ok(())
}

pub(crate) fn source_snapshots(store: &Store, run_id: &str) -> Result<Vec<Value>> {
    Ok(store
        .documents("profile-inputs")?
        .into_iter()
        .filter(|d| d["run_id"] == run_id)
        .flat_map(|d| {
            d["source_snapshots"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_raw_or_sidecar_never_publishes_a_mismatched_projection() {
        for changed in ["cpu.json", "cpu.syms.json"] {
            let input = tempfile::tempdir().unwrap();
            let data = tempfile::tempdir().unwrap();
            for name in ["cpu.json", "cpu.syms.json"] {
                fs::copy(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("src/importers/fixtures/native-reload")
                        .join(name),
                    input.path().join(name),
                )
                .unwrap();
            }
            let store = Store::open(data.path()).unwrap();
            let run = serde_json::from_value(json!({"id":"stable-inputs","name":"control","status":"completed","command":[],"cwd":"/removed","runtime":"native","started_at_us":1,"finished_at_us":2,"exit_code":0,"diagnostics":[],"source":{},"correctness":{}})).unwrap();
            store.save_run(&run).unwrap();
            let profile = crate::importers::import(
                &input.path().join("cpu.json"),
                "firefox",
                "stable-inputs",
            )
            .unwrap();
            fs::write(input.path().join(changed), b"replaced after parsing").unwrap();
            let error = publish(&store, "stable-inputs", profile).err().unwrap();
            assert!(
                error.to_string().contains("changed during normalization"),
                "{error:#}"
            );
            assert!(store.records("stable-inputs").unwrap().is_empty());
        }
    }
}

#[cfg(test)]
mod input_tests;

use crate::{importers::ImportedProfile, model::runs::Artifact, storage::Store};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

pub(crate) struct Publication {
    pub accepted: bool,
    pub diagnostics: Vec<String>,
}

pub(crate) fn publish(
    store: &Store,
    run_id: &str,
    profile: ImportedProfile,
) -> Result<Publication> {
    let raw = store.add_artifact(run_id, &profile.source, &profile.format)?;
    let mut sidecars = Vec::new();
    for path in &profile.sidecars {
        sidecars.push(store.add_artifact(run_id, path, "profile-sidecar")?);
    }
    for image in &profile.binary_snapshots {
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
    }
    let context = profile
        .native_image_context
        .as_ref()
        .map(|value| {
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
    }
    let id = format!("{run_id}:{}", raw.sha256);
    let document = json!({"run_id":run_id,"raw":raw,"sidecars":sidecars,"native_context_artifact_id":context.map(|a|a.id),"source_snapshots":sources,"normalization_fingerprint":profile.input_fingerprint,"status":"accepted"});
    let accepted = store.publish(
        run_id,
        &format!("import-v1:{}", raw.sha256),
        &format!("normalization:{}", profile.input_fingerprint),
        &profile.records,
        Some(("profile-inputs", &id, &document)),
    )?;
    Ok(Publication {
        accepted,
        diagnostics: profile.diagnostics,
    })
}

pub(crate) fn reanalyze(store: &Store, run_id: &str, artifact_id: &str) -> Result<Publication> {
    let mut components = Path::new(run_id).components();
    anyhow::ensure!(
        matches!(components.next(), Some(std::path::Component::Normal(_)))
            && components.next().is_none(),
        "recording ID cannot name a managed directory"
    );
    let raw = store
        .artifacts(run_id)?
        .into_iter()
        .find(|a| a.id == artifact_id)
        .context("recording artifact not found")?;
    let inputs = store.document("profile-inputs",&format!("{run_id}:{}",raw.sha256)).context("recording lacks a retained import manifest; import the original recording and its sidecars first")?;
    let raw: Artifact = serde_json::from_value(inputs["raw"].clone())?;
    let directory = store
        .root()
        .join("captures")
        .join(run_id)
        .join("reanalysis")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&directory)?;
    let path = restore(store, run_id, &raw, &directory)?;
    for sidecar in inputs["sidecars"]
        .as_array()
        .context("retained sidecar manifest missing")?
    {
        restore(
            store,
            run_id,
            &serde_json::from_value(sidecar.clone())?,
            &directory,
        )?;
    }
    let profile = if let Some(id) = inputs["native_context_artifact_id"].as_str() {
        crate::importers::import_with_native_context(
            &path,
            &raw.format,
            run_id,
            &store.artifact_path(run_id, id)?,
        )?
    } else {
        crate::importers::import(&path, &raw.format, run_id)?
    };
    publish(store, run_id, profile)
}

fn restore(
    store: &Store,
    run_id: &str,
    artifact: &Artifact,
    directory: &Path,
) -> Result<std::path::PathBuf> {
    anyhow::ensure!(
        !artifact.name.is_empty()
            && !artifact.name.contains(['/', '\\', '\0'])
            && !matches!(artifact.name.as_str(), "." | ".."),
        "invalid retained artifact filename"
    );
    let content = fs::read(store.artifact_path(run_id, &artifact.id)?)?;
    anyhow::ensure!(
        format!("{:x}", Sha256::digest(&content)) == artifact.sha256,
        "retained profile/sidecar integrity mismatch"
    );
    let path = directory.join(&artifact.name);
    fs::write(&path, content)?;
    Ok(path)
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

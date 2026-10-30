//! Preserve normalized evidence and publish its immutable input manifest.
use super::{Publication, checkpoint};
use crate::{
    importers::ImportedProfile,
    publication::{PublicationMetadata, PublicationRequest},
    storage::Store,
};
use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Instant;

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

pub(super) fn publish_with_limits(
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

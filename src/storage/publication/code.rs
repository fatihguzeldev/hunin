//! Code proof custody before admission, then typed references under the writer snapshot.
use super::*;
use crate::model::{
    code::{self, ArtifactRef, CodeInstance, CodeRecord, InstanceKind, RangeSpace},
    runs::Artifact,
};
use anyhow::{Context, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
};

// This streams unique proof blobs; the separate analyzer JSON input limit is 128 MiB.
const PROOF_BYTES: u64 = 1024 * 1024 * 1024;

pub(super) fn prepare_artifacts(
    store: &Store,
    run_id: &str,
    records: &[Value],
    checkpoint: &mut impl FnMut() -> Result<()>,
) -> Result<HashMap<String, Artifact>> {
    let mut artifacts = HashMap::new();
    let mut hashed = HashSet::new();
    let mut read_bytes = 0u64;
    for record in records {
        checkpoint()?;
        if let Some(code) = code::parse_record(record)? {
            for proof in code.artifact_refs() {
                let artifact = store.artifact_metadata(run_id, &proof.artifact_id)?;
                validate_artifact(proof, &artifact)?;
                if hashed.insert(artifact.sha256.clone()) {
                    ensure!(
                        artifact.size <= PROOF_BYTES.saturating_sub(read_bytes),
                        "code-proof-bytes-v1 exceeds the 1 GiB publication limit"
                    );
                    let path = store.root().join("artifacts").join(&artifact.sha256);
                    ensure!(
                        std::fs::metadata(&path)?.is_file(),
                        "code proof artifact is not a regular file"
                    );
                    let mut file = std::fs::File::open(path)?;
                    ensure!(
                        file.metadata()?.is_file(),
                        "opened code proof artifact is not a regular file"
                    );
                    let mut hasher = Sha256::new();
                    let mut size = 0u64;
                    let mut buffer = [0u8; 64 * 1024];
                    loop {
                        checkpoint()?;
                        let capacity = artifact
                            .size
                            .saturating_sub(size)
                            .saturating_add(1)
                            .min(buffer.len() as u64)
                            as usize;
                        let count = file.read(&mut buffer[..capacity])?;
                        if count == 0 {
                            break;
                        }
                        size = size
                            .checked_add(count as u64)
                            .context("code proof size overflow")?;
                        ensure!(
                            size <= artifact.size,
                            "code proof artifact grew beyond its retained identity"
                        );
                        hasher.update(&buffer[..count]);
                    }
                    ensure!(
                        size == artifact.size
                            && format!("{:x}", hasher.finalize()) == artifact.sha256,
                        "code proof retained bytes disagree with their artifact identity"
                    );
                    read_bytes += size;
                }
                artifacts.insert(artifact.id.clone(), artifact);
            }
        }
    }
    Ok(artifacts)
}

fn validate_artifact(proof: &ArtifactRef, artifact: &Artifact) -> Result<()> {
    ensure!(
        proof.sha256 == artifact.sha256,
        "code proof digest disagrees with the run-owned artifact"
    );
    if let (Some(offset), Some(length)) = (proof.offset, proof.length) {
        ensure!(
            offset
                .checked_add(length)
                .is_some_and(|end| end <= artifact.size),
            "code proof byte range exceeds retained artifact bounds"
        );
    }
    Ok(())
}

pub(super) fn validate(
    connection: &Connection,
    run_id: &str,
    records: &[Value],
    artifacts: &HashMap<String, Artifact>,
    checkpoint: &mut impl FnMut() -> Result<()>,
) -> Result<()> {
    let batch: HashMap<_, _> = records
        .iter()
        .filter_map(|record| record["id"].as_str().map(|id| (id, record)))
        .collect();
    let lookup = |id: &str| -> Result<Value> {
        if let Some(record) = batch.get(id) {
            return Ok((*record).clone());
        }
        let text: String = connection.query_row("SELECT o.record FROM observations o JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND o.id=?2 AND r.active=1",params![run_id,id],|row|row.get(0)).with_context(|| format!("code reference is not selected: {id}"))?;
        ensure!(
            text.len() <= 8 * 1024 * 1024,
            "code reference exceeds the 8 MiB record limit"
        );
        Ok(serde_json::from_str(&text)?)
    };
    let instance = |id: &str| -> Result<(Value, CodeInstance)> {
        let record = lookup(id)?;
        let Some(CodeRecord::Instance(instance)) = code::parse_record(&record)? else {
            anyhow::bail!("code reference must identify a code.instance observation: {id}")
        };
        Ok((record, instance))
    };
    for record in records {
        checkpoint()?;
        if let Some(code) = code::parse_record(record)? {
            if record["basis"] == "derived" {
                ensure!(
                    record["evidence"]
                        .as_array()
                        .is_some_and(|values| !values.is_empty()),
                    "derived code facts need observation evidence"
                );
            }
            for proof in code.artifact_refs() {
                let (metadata, indexed_sha): (String, String) = connection.query_row(
                    "SELECT metadata,sha256 FROM artifacts WHERE run_id=?1 AND id=?2",
                    params![run_id, proof.artifact_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                let artifact = crate::storage::artifacts::decode_metadata(
                    &metadata,
                    &indexed_sha,
                    &proof.artifact_id,
                )?;
                validate_artifact(proof, &artifact)?;
                let verified = artifacts
                    .get(&proof.artifact_id)
                    .context("code artifact proof was not verified before publication")?;
                ensure!(
                    verified.sha256 == artifact.sha256 && verified.size == artifact.size,
                    "code artifact identity changed before publication"
                );
            }
            match code {
                CodeRecord::Instance(instance) => {
                    if let (Some(range), Some(proof)) = (&instance.range, &instance.artifact)
                        && range.space == RangeSpace::ArtifactOffset
                    {
                        let artifact = artifacts
                            .get(&proof.artifact_id)
                            .context("code artifact proof missing")?;
                        let start = code::hex(&range.start)?;
                        let end = code::hex(&range.end_exclusive)?;
                        ensure!(
                            end <= artifact.size,
                            "code instance range exceeds its artifact"
                        );
                        if let (Some(offset), Some(length)) = (proof.offset, proof.length) {
                            ensure!(
                                start >= offset && end <= offset + length,
                                "code instance range exceeds its proof slice"
                            );
                        }
                    }
                }
                CodeRecord::Mapping(mapping) => {
                    for (id, range) in [
                        (
                            record["from"].as_str().context("mapping from missing")?,
                            &mapping.from_range,
                        ),
                        (
                            record["to"].as_str().context("mapping to missing")?,
                            &mapping.to_range,
                        ),
                    ] {
                        let (_, endpoint) = instance(id)?;
                        ensure!(
                            endpoint
                                .range
                                .as_ref()
                                .context("mapping endpoint needs a declared code range")?
                                .contains_range(range)?,
                            "mapping range is outside its endpoint code instance"
                        );
                    }
                    if mapping.proof.verification != code::Verification::Declared {
                        ensure!(
                            !mapping.proof.artifact_refs.is_empty(),
                            "identity/semantic checked mapping claims need retained artifact proof"
                        );
                    }
                }
                CodeRecord::Unavailable(unavailable) => {
                    let anchor = lookup(&unavailable.anchor.observation_id)?;
                    ensure!(anchor["kind"]=="sample" && anchor["stack"].as_array().is_some_and(|frames|unavailable.anchor.frame_index<frames.len()),"code unavailability must anchor an existing sampled frame");
                }
            }
        }
        for frame in record["stack"].as_array().into_iter().flatten() {
            if let Some(id) = frame["code_instance_id"].as_str() {
                let (owner, instance) = instance(id)?;
                ensure!(
                    instance.instance_kind == InstanceKind::Runtime
                        && owner["pid"] == record["pid"],
                    "sampled code instance must belong to this runtime process"
                );
                if let Some(offset) = frame["code_offset"].as_str() {
                    ensure!(
                        instance
                            .range
                            .as_ref()
                            .context("sampled code offset needs an instance range")?
                            .contains(code::hex(offset)?)?,
                        "sampled code offset is outside its code instance"
                    );
                }
                if let Some(validity) = &instance.validity {
                    ensure!(
                        validity.at_in(
                            record["timestamp_us"]
                                .as_u64()
                                .context("sample timestamp missing")?,
                            &record["attributes"]
                        ) != Some(false),
                        "sample lies outside its code instance lifetime"
                    );
                }
            }
        }
    }
    Ok(())
}

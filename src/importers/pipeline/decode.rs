//! Coordinate format-specific interpretation and collect its retained evidence.
use crate::importers::{
    ImportedProfile, firefox,
    formats::ProfileFormat,
    go_trace, graal, inputs, javascript_sources, jsonl,
    output::{self, OutputBudget},
    pprof, python_cpu, v8, vmprof,
};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::path::Path;

pub(super) fn decode_snapshot(
    path: &Path,
    format: &str,
    run_id: &str,
    native_context: Option<&Path>,
    snapshot: &inputs::Snapshot,
    mut digest: Sha256,
    budget: &mut OutputBudget,
) -> Result<ImportedProfile> {
    let kind = ProfileFormat::parse(format)?;
    let raw_sha256 = snapshot.raw_sha256().to_owned();
    let sidecar_sha256 = snapshot.sidecar_sha256();
    let sidecars = sidecar_sha256.keys().cloned().collect();
    let decoder_path = snapshot.path();
    let raw = snapshot.decoded()?;
    let firefox = (kind == ProfileFormat::Firefox)
        .then(|| {
            firefox::Input::load(
                decoder_path,
                &raw,
                &raw_sha256,
                sidecar_sha256
                    .get(&path.with_extension("lifetimes.json"))
                    .map(String::as_str),
                native_context,
                budget,
            )
        })
        .transpose()?;
    if let Some(input) = &firefox {
        digest.update(b"native-image-identifiers-v1\0");
        digest.update(serde_json::to_vec(&input.image_fingerprint())?);
    }
    let vmprof = (kind == ProfileFormat::Vmprof)
        .then(|| {
            vmprof::read(
                decoder_path,
                snapshot.remaining(std::time::Duration::from_secs(60))?,
            )
        })
        .transpose()?;
    let go_trace = (kind == ProfileFormat::GoTrace)
        .then(|| go_trace::read(decoder_path, snapshot.deadline()))
        .transpose()?;
    let decoder_provenance = vmprof
        .as_ref()
        .map(|decoded| decoded.provenance.clone())
        .or_else(|| go_trace.as_ref().map(|decoded| decoded.provenance.clone()));
    if let Some(provenance) = &decoder_provenance {
        digest.update(b"external-decoder-identity-v1\0");
        digest.update(serde_json::to_vec(&provenance["identity"])?);
    }
    let key = format!("{:x}", digest.finalize());
    let scripts = if kind == ProfileFormat::V8 {
        Some(javascript_sources::Scripts::load(decoder_path)?)
    } else {
        None
    };
    let mut records = output::RecordBatch::new(budget, snapshot.deadline());
    let diagnostics = match kind {
        ProfileFormat::Firefox => {
            firefox
                .as_ref()
                .unwrap()
                .decode_into(decoder_path, run_id, &key, &mut records)?
        }
        ProfileFormat::V8 => v8::decode_into(
            decoder_path,
            &serde_json::from_slice(&raw)?,
            run_id,
            &key,
            scripts.as_ref().unwrap(),
            &mut records,
        )?,
        ProfileFormat::Graal => graal::decode_into(
            decoder_path,
            &serde_json::from_slice(&raw)?,
            run_id,
            &key,
            &mut records,
        )?,
        ProfileFormat::Pprof => pprof::decode_into(&raw, run_id, &key, &mut records)?,
        ProfileFormat::PythonCpuProgress => {
            python_cpu::decode_into(decoder_path, run_id, &key, &mut records)?
        }
        ProfileFormat::GoTrace => {
            go_trace::decode_into(go_trace.as_ref().unwrap(), run_id, &key, &mut records)?
        }
        ProfileFormat::Vmprof => {
            vmprof::decode_into(vmprof.as_ref().unwrap(), run_id, &key, &mut records)?
        }
        ProfileFormat::Jsonl => {
            jsonl::decode_into(&raw, run_id, &mut records)?;
            Vec::new()
        }
    };
    let (native_image_context, binary_snapshots) = firefox
        .map(|input| {
            let (context, snapshots) = input.into_evidence(&raw_sha256);
            (Some(context), snapshots)
        })
        .unwrap_or_default();
    snapshot.verify()?;
    let profile = ImportedProfile {
        source: path.to_owned(),
        raw_sha256,
        format: format.to_owned(),
        records: records.finish()?,
        diagnostics,
        input_fingerprint: key,
        sidecars,
        sidecar_sha256,
        source_snapshots: scripts
            .map(javascript_sources::Scripts::into_snapshots)
            .unwrap_or_default(),
        binary_snapshots,
        native_image_context,
        decoder_provenance,
    };
    budget.profile(&profile)?;
    Ok(profile)
}

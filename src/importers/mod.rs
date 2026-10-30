mod firefox;
mod formats;
use formats::ProfileFormat;
pub(crate) use formats::import_formats;
mod go_trace;
mod graal;
mod inputs;
pub(crate) use inputs::{Limits as NormalizationInputLimits, NormalizationByteLimit};
mod javascript_sources;
mod output;
#[cfg(test)]
use output::OutputDimension;
pub(crate) use output::{NormalizationOutputLimit, OutputBudget};
mod pprof;
mod result;
pub use result::{BinarySnapshot, ImportedProfile, SourceSnapshot};
mod python_cpu;
mod v8;
mod vmprof;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub fn import(path: &Path, format: &str, run_id: &str) -> Result<ImportedProfile> {
    import_controlled(path, format, run_id, None)
}
pub fn import_with_native_context(
    path: &Path,
    format: &str,
    run_id: &str,
    manifest_path: &Path,
) -> Result<ImportedProfile> {
    import_with_native_context_controlled(path, format, run_id, manifest_path, None)
}

pub(crate) fn import_with_native_context_controlled(
    path: &Path,
    format: &str,
    run_id: &str,
    manifest_path: &Path,
    deadline: Option<std::time::Instant>,
) -> Result<ImportedProfile> {
    import_with_native_context_budget_controlled(
        path,
        format,
        run_id,
        manifest_path,
        &mut OutputBudget::default(),
        deadline,
    )
}

pub(crate) fn import_controlled(
    path: &Path,
    format: &str,
    run_id: &str,
    deadline: Option<std::time::Instant>,
) -> Result<ImportedProfile> {
    import_with_limits(
        path,
        format,
        run_id,
        None,
        NormalizationInputLimits::default(),
        deadline,
    )
}

fn import_with_limits(
    path: &Path,
    format: &str,
    run_id: &str,
    native_context: Option<&Path>,
    limits: inputs::Limits,
    deadline: Option<std::time::Instant>,
) -> Result<ImportedProfile> {
    import_budget_with_limits(
        path,
        format,
        run_id,
        native_context,
        limits,
        &mut OutputBudget::default(),
        deadline,
    )
}

pub(crate) fn import_with_budget_controlled(
    path: &Path,
    format: &str,
    run_id: &str,
    budget: &mut OutputBudget,
    deadline: Option<std::time::Instant>,
) -> Result<ImportedProfile> {
    import_budget_with_limits(
        path,
        format,
        run_id,
        None,
        NormalizationInputLimits::default(),
        budget,
        deadline,
    )
}

pub(crate) fn import_with_native_context_budget_controlled(
    path: &Path,
    format: &str,
    run_id: &str,
    manifest_path: &Path,
    budget: &mut OutputBudget,
    deadline: Option<std::time::Instant>,
) -> Result<ImportedProfile> {
    anyhow::ensure!(
        ProfileFormat::parse(format)? == ProfileFormat::Firefox,
        "native image context requires a Firefox/samply recording"
    );
    import_budget_with_limits(
        path,
        format,
        run_id,
        Some(manifest_path),
        NormalizationInputLimits::default(),
        budget,
        deadline,
    )
}

fn import_budget_with_limits(
    path: &Path,
    format: &str,
    run_id: &str,
    native_context: Option<&Path>,
    limits: inputs::Limits,
    budget: &mut OutputBudget,
    deadline: Option<std::time::Instant>,
) -> Result<ImportedProfile> {
    budget.set_deadline(deadline);
    let deadline = budget.deadline();
    budget.check()?;
    let (snapshot, digest) = capture_inputs(path, format, limits, deadline)?;
    decode_snapshot(
        path,
        format,
        run_id,
        native_context,
        &snapshot,
        digest,
        budget,
    )
}

fn capture_inputs(
    path: &Path,
    format: &str,
    limits: inputs::Limits,
    deadline: Option<std::time::Instant>,
) -> Result<(inputs::Snapshot, Sha256)> {
    let kind = ProfileFormat::parse(format)?;
    let mut snapshot = inputs::Snapshot::new(limits, deadline)?;
    let mut digest = Sha256::new();
    digest.update(b"hunin-import-v9\0");
    digest.update(format.as_bytes());
    digest.update([0]);
    snapshot.capture_raw(path, &mut digest)?;
    if kind == ProfileFormat::V8 {
        digest.update(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .as_bytes(),
        );
    }
    let scripts = if kind == ProfileFormat::V8 {
        v8::script_paths(path)
    } else {
        vec![]
    };
    for sidecar in sidecars(path, kind) {
        digest.update([0]);
        digest.update(
            sidecar
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .as_bytes(),
        );
        let maximum = if scripts.contains(&sidecar) {
            javascript_sources::MAX_TOTAL
        } else if kind == ProfileFormat::Firefox && sidecar == path.with_extension("lifetimes.json")
        {
            firefox::MAX_LIFETIME_SIDECAR
        } else {
            limits.sidecar_bytes
        };
        snapshot.capture_sidecar(&sidecar, maximum, &mut digest)?;
    }
    Ok((snapshot, digest))
}

fn decode_snapshot(
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
            for (line, text) in std::str::from_utf8(&raw)?.lines().enumerate() {
                if text.trim().is_empty() {
                    continue;
                }
                let record: Value = serde_json::from_str(text)
                    .with_context(|| format!("JSONL line {}", line + 1))?;
                if record["run_id"].as_str() != Some(run_id) {
                    bail!("JSONL record belongs to another run");
                }
                records.push(record)?;
            }
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

fn sidecars(path: &Path, format: ProfileFormat) -> Vec<PathBuf> {
    let candidates = match format {
        ProfileFormat::Firefox => vec![
            path.with_extension("syms.json"),
            path.with_extension("lifetimes.json"),
        ],
        ProfileFormat::V8 => v8::metadata_paths(path)
            .into_iter()
            .chain(v8::script_paths(path))
            .collect(),
        ProfileFormat::Graal => vec![path.with_extension("meta.json")],
        ProfileFormat::Vmprof => vec![PathBuf::from(format!("{}.meta.json", path.display()))],
        ProfileFormat::PythonCpuProgress => {
            vec![PathBuf::from(format!("{}.meta.json", path.display()))]
        }
        ProfileFormat::GoTrace => vec![
            path.parent()
                .unwrap_or_else(|| Path::new("."))
                .join("go-tool.json"),
        ],
        ProfileFormat::Pprof | ProfileFormat::Jsonl => Vec::new(),
    };
    let mut result = candidates
        .into_iter()
        .filter(|p| p.is_file())
        .collect::<Vec<_>>();
    result.sort();
    result.dedup();
    result
}

#[cfg(test)]
mod recorded_controls {
    use super::*;
    use std::{collections::HashSet, fs};

    #[test]
    fn real_native_recording_resolves_known_hotspot_and_validates() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/importers/fixtures/native.json");
        let profile = import(&path, "firefox", "control").unwrap();
        assert!(profile.records.len() > 100);
        assert!(
            profile
                .records
                .iter()
                .flat_map(|r| r["stack"].as_array().into_iter().flatten())
                .any(|f| f["function"] == "hunin_cpu_hotspot"
                    && f["file"]
                        .as_str()
                        .is_some_and(|s| s.ends_with("native_cpu.c")))
        );
        crate::ingestion::validate("control", &profile.records, &HashSet::new()).unwrap();
        let replay = import(&path, "firefox", "control").unwrap();
        assert_eq!(
            profile.records, replay.records,
            "reimport must keep stable observation identity"
        );
    }

    #[test]
    fn real_graal_recording_retains_epoch_sample_times() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/importers/fixtures/graal.json");
        let profile = import(&path, "graal-cpusampler", "control").unwrap();
        assert!(
            profile
                .records
                .iter()
                .all(|r| r["timestamp_us"].as_u64().unwrap() > 1_700_000_000_000_000)
        );
        assert!(
            profile
                .records
                .iter()
                .flat_map(|r| r["stack"].as_array().unwrap())
                .any(|f| f["function"] == "hunin_python_hotspot")
        );
        crate::ingestion::validate("control", &profile.records, &HashSet::new()).unwrap();
    }

    #[test]
    fn real_go_pprof_preserves_cpu_time_and_source_hotspot() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/importers/fixtures/go.pprof");
        let profile = import(&path, "pprof", "control").unwrap();
        assert!(
            profile
                .records
                .iter()
                .all(|r| r["weight_unit"] == "microseconds"
                    && r["attributes"]["temporal_resolution"] == "aggregate")
        );
        assert!(
            profile
                .records
                .iter()
                .flat_map(|r| r["stack"].as_array().unwrap())
                .any(|f| f["function"]
                    .as_str()
                    .is_some_and(|n| n.ends_with("huninGoHotspot")))
        );
        crate::ingestion::validate("control", &profile.records, &HashSet::new()).unwrap();
    }

    #[test]
    fn symbol_sidecar_changes_input_identity_and_resolves_reanalysis() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/importers/fixtures");
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.json");
        fs::copy(root.join("native.json"), &path).unwrap();
        let before = import(&path, "firefox", "control").unwrap();
        fs::copy(
            root.join("native.syms.json"),
            path.with_extension("syms.json"),
        )
        .unwrap();
        let after = import(&path, "firefox", "control").unwrap();
        assert_ne!(before.input_fingerprint, after.input_fingerprint);
        assert_ne!(before.records[0]["id"], after.records[0]["id"]);
        assert!(
            after
                .records
                .iter()
                .flat_map(|r| r["stack"].as_array().into_iter().flatten())
                .any(|f| f["function"] == "hunin_cpu_hotspot")
        );
        assert_eq!(after.sidecars.len(), 1);
    }

    #[test]
    fn deno_worker_clock_anchor_does_not_merge_worker_identity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory
            .path()
            .join("CPU.1792774105123.42.worker-1.cpuprofile");
        fs::write(&path,serde_json::to_vec(&json!({"startTime":10,"nodes":[{"id":1,"callFrame":{"functionName":"workerHot"}}],"samples":[1],"timeDeltas":[3]})).unwrap()).unwrap();
        fs::write(directory.path().join("clock-42-0.meta.json"),r#"{"pid":42,"tid":"0","profile_time_origin_us":1700000000000000,"clock_source":"control"}"#).unwrap();
        let profile = import(&path, "v8-cpuprofile", "control").unwrap();
        assert_eq!(profile.records[0]["tid"], "worker-1");
        assert_eq!(profile.records[0]["pid"], 42);
        assert_eq!(profile.records[0]["timestamp_us"], 1700000000000013u64);
        crate::ingestion::validate("control", &profile.records, &HashSet::new()).unwrap();
    }
}

#[cfg(test)]
mod input_budget_tests;

#[cfg(test)]
mod output_budget_tests;

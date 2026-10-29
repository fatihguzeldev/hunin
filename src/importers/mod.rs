mod firefox;
mod go_trace;
mod graal;
mod javascript_sources;
mod native_identity;
mod native_lifetimes;
mod native_sampling;
mod pprof;
mod python_cpu;
mod v8;
mod vmprof;

use crate::model::observations::StackSample;
use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct SourceSnapshot {
    pub uri: String,
    pub content: String,
    pub sha256: String,
    pub provenance: Value,
}
#[derive(Debug, Clone)]
pub struct BinarySnapshot {
    pub sha256: String,
    pub content: Vec<u8>,
}
#[derive(Debug, Clone)]
pub struct ImportedProfile {
    pub source: PathBuf,
    pub raw_sha256: String,
    pub format: String,
    pub records: Vec<Value>,
    pub diagnostics: Vec<String>,
    pub input_fingerprint: String,
    pub sidecars: Vec<PathBuf>,
    pub sidecar_sha256: BTreeMap<PathBuf, String>,
    pub source_snapshots: Vec<SourceSnapshot>,
    pub binary_snapshots: Vec<BinarySnapshot>,
    pub native_image_context: Option<Value>,
    /// Actual offline decoder identity and resolved execution paths, not capture identity.
    pub decoder_provenance: Option<Value>,
}

pub fn import(path: &Path, format: &str, run_id: &str) -> Result<ImportedProfile> {
    import_inner(path, format, run_id, None)
}
pub fn import_with_native_context(
    path: &Path,
    format: &str,
    run_id: &str,
    manifest_path: &Path,
) -> Result<ImportedProfile> {
    anyhow::ensure!(
        matches!(format, "firefox" | "firefox-profile" | "samply"),
        "native image context requires a Firefox/samply recording"
    );
    import_inner(path, format, run_id, Some(manifest_path))
}
fn import_inner(
    path: &Path,
    format: &str,
    run_id: &str,
    native_context: Option<&Path>,
) -> Result<ImportedProfile> {
    let bytes = fs::read(path).with_context(|| format!("read profile {}", path.display()))?;
    let raw_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let sidecars = sidecars(path, format);
    let mut digest = Sha256::new();
    digest.update(b"hunin-import-v9\0");
    digest.update(format.as_bytes());
    digest.update([0]);
    digest.update(&bytes);
    if matches!(format, "v8" | "v8-cpuprofile" | "cpuprofile") {
        digest.update(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .as_bytes(),
        );
    }
    let mut sidecar_sha256 = BTreeMap::new();
    for sidecar in &sidecars {
        digest.update([0]);
        digest.update(
            sidecar
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .as_bytes(),
        );
        let content = fs::read(sidecar)?;
        sidecar_sha256.insert(sidecar.clone(), format!("{:x}", Sha256::digest(&content)));
        digest.update(content);
    }
    let raw = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoded = Vec::new();
        GzDecoder::new(bytes.as_slice())
            .take(512 * 1024 * 1024 + 1)
            .read_to_end(&mut decoded)?;
        if decoded.len() > 512 * 1024 * 1024 {
            bail!("profile exceeds 512 MiB decompressed limit");
        }
        decoded
    } else {
        bytes
    };
    let mut native_raw = if matches!(format, "firefox" | "firefox-profile" | "samply") {
        Some(serde_json::from_slice::<Value>(&raw)?)
    } else {
        None
    };
    let lifetimes = if let Some(raw) = &native_raw {
        let sidecar = path.with_extension("lifetimes.json");
        native_lifetimes::Timeline::load(
            &sidecar,
            &raw_sha256,
            sidecar_sha256.get(&sidecar).map(String::as_str),
            raw,
        )?
    } else {
        None
    };
    if let (Some(timeline), Some(raw)) = (&lifetimes, &mut native_raw) {
        let libraries = raw["libs"]
            .as_array_mut()
            .context("native library dictionary missing")?;
        for library in timeline.libraries() {
            if !libraries.contains(library) {
                libraries.push(library.clone());
            }
        }
    }
    let images = native_raw
        .as_ref()
        .map(|raw| match native_context {
            Some(path) => native_identity::Images::from_context(raw, path, &raw_sha256),
            None => Ok(native_identity::Images::inspect(raw)),
        })
        .transpose()?;
    if let Some(images) = &images {
        digest.update(b"native-image-identifiers-v1\0");
        digest.update(serde_json::to_vec(&images.fingerprint())?);
    }
    let vmprof = (format == "vmprof")
        .then(|| vmprof::read(path))
        .transpose()?;
    let decoder_provenance = vmprof.as_ref().map(|decoded| decoded.provenance.clone());
    if let Some(provenance) = &decoder_provenance {
        digest.update(b"external-decoder-identity-v1\0");
        digest.update(serde_json::to_vec(&provenance["identity"])?);
    }
    let key = format!("{:x}", digest.finalize());
    let scripts = if matches!(format, "v8" | "v8-cpuprofile" | "cpuprofile") {
        Some(javascript_sources::Scripts::load(path)?)
    } else {
        None
    };
    let (records, mut diagnostics) = match format {
        "firefox" | "firefox-profile" | "samply" => firefox::decode_with_lifetimes(
            path,
            native_raw.as_ref().unwrap(),
            run_id,
            &key,
            images.as_ref().unwrap(),
            lifetimes.as_ref(),
        )?,
        "v8" | "v8-cpuprofile" | "cpuprofile" => v8::decode(
            path,
            &serde_json::from_slice(&raw)?,
            run_id,
            &key,
            scripts.as_ref().unwrap(),
        )?,
        "graal" | "graal-cpusampler" => {
            graal::decode(path, &serde_json::from_slice(&raw)?, run_id, &key)?
        }
        "pprof" | "go-pprof" => pprof::decode(&raw, run_id, &key)?,
        "python-cpu-progress" => python_cpu::decode(path, run_id, &key)?,
        "go-trace" => go_trace::decode(path, run_id, &key)?,
        "vmprof" => vmprof::decode(vmprof.as_ref().unwrap(), run_id, &key)?,
        "hunin-jsonl" | "jsonl" => {
            let mut records = Vec::new();
            for (line, text) in std::str::from_utf8(&raw)?.lines().enumerate() {
                if text.trim().is_empty() {
                    continue;
                }
                let record: Value = serde_json::from_str(text)
                    .with_context(|| format!("JSONL line {}", line + 1))?;
                if record["run_id"].as_str() != Some(run_id) {
                    bail!("JSONL record belongs to another run");
                }
                records.push(record);
            }
            (records, Vec::new())
        }
        other => bail!(
            "unsupported profile format {other}; expected v8-cpuprofile, firefox, vmprof, graal-cpusampler, pprof or hunin-jsonl"
        ),
    };
    if lifetimes.is_none()
        && native_raw.as_ref().is_some_and(|raw| {
            raw["meta"]["oscpu"]
                .as_str()
                .is_some_and(|os| os.to_ascii_lowercase().starts_with("windows"))
        })
    {
        diagnostics.push(native_sampling::LEGACY_DIAGNOSTIC.into());
    }
    let native_image_context = images.as_ref().map(|images| images.context(&raw_sha256));
    let binary_snapshots = images
        .map(native_identity::Images::into_snapshots)
        .unwrap_or_default();
    Ok(ImportedProfile {
        source: path.to_owned(),
        raw_sha256,
        format: format.to_owned(),
        records,
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
    })
}

fn sidecars(path: &Path, format: &str) -> Vec<PathBuf> {
    let candidates = match format {
        "firefox" | "firefox-profile" | "samply" => vec![
            path.with_extension("syms.json"),
            path.with_extension("lifetimes.json"),
        ],
        "v8" | "v8-cpuprofile" | "cpuprofile" => v8::metadata_paths(path)
            .into_iter()
            .chain(v8::script_paths(path))
            .collect(),
        "graal" | "graal-cpusampler" => vec![path.with_extension("meta.json")],
        "vmprof" => vec![PathBuf::from(format!("{}.meta.json", path.display()))],
        "python-cpu-progress" => vec![PathBuf::from(format!("{}.meta.json", path.display()))],
        "go-trace" => vec![
            path.parent()
                .unwrap_or_else(|| Path::new("."))
                .join("go-tool.json"),
        ],
        _ => Vec::new(),
    };
    let mut result = candidates
        .into_iter()
        .filter(|p| p.is_file())
        .collect::<Vec<_>>();
    result.sort();
    result.dedup();
    result
}

pub(crate) fn sample(run_id: &str, key: &str, index: usize, sample: StackSample<'_>) -> Value {
    let StackSample {
        pid,
        tid,
        timestamp_us,
        stack,
        weight,
        weight_unit: unit,
        attributes,
    } = sample;
    json!({"schema_version":1,"kind":"sample","id":format!("profile:{key}:{index}"),
        "run_id":run_id,"producer_id":format!("profile:{key}:{pid}"),"pid":pid,"tid":tid,
        "timestamp_us":timestamp_us,"basis":"sampled","stack":stack,"weight":weight,
        "weight_unit":unit,"attributes":attributes})
}

pub(crate) fn checked_us(value: f64, description: &str) -> Result<u64> {
    if !value.is_finite() || !(0.0..=9_007_199_254_740_991.0).contains(&value) {
        bail!("invalid {description}: {value}");
    }
    Ok(value.round() as u64)
}

#[cfg(test)]
mod recorded_controls {
    use super::*;
    use std::collections::HashSet;

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

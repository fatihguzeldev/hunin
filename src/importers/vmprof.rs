use super::sample;
use crate::{
    host::{assets, probe, tool},
    model::observations::{SampleWeightUnit, StackSample},
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Deserialize)]
struct Provenance {
    schema_version: u32,
    identity: Identity,
    evidence: Evidence,
}

#[derive(Deserialize)]
struct Evidence {
    script: String,
    executable: String,
    resolved_executable: String,
    package_root: String,
    extension_paths: BTreeMap<String, String>,
    scope: String,
}

#[derive(Deserialize)]
struct Identity {
    decoder: String,
    script_sha256: String,
    runtime: Runtime,
    package: Package,
}

#[derive(Deserialize)]
struct Runtime {
    implementation: String,
    version: (u64, u64, u64, String, u64),
    executable_sha256: String,
    byteorder: String,
    default_text_encoding: String,
    flags: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    version: String,
    python_files: BTreeMap<String, String>,
    loaded_extensions: BTreeMap<String, String>,
}

fn validate_provenance(value: &Value) -> Result<()> {
    let receipt: Provenance = serde_json::from_value(value.clone()).context(
        "incomplete vmprof decoder provenance; rebuild or reinstall matching decoder assets",
    )?;
    let identity = &receipt.identity;
    let digest =
        |value: &str| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    anyhow::ensure!(
        receipt.schema_version == 1
            && identity.decoder == "hunin-vmprof-v1"
            && digest(&identity.script_sha256)
            && !identity.runtime.implementation.is_empty()
            && identity.runtime.version.0 > 0
            && !identity.runtime.version.3.is_empty()
            && digest(&identity.runtime.executable_sha256)
            && matches!(identity.runtime.byteorder.as_str(), "little" | "big")
            && !identity.runtime.default_text_encoding.is_empty()
            && identity
                .runtime
                .flags
                .get("optimize")
                .is_some_and(Value::is_u64)
            && identity.package.name == "vmprof"
            && !identity.package.version.is_empty()
            && identity.package.python_files.contains_key("reader.py")
            && identity
                .package
                .python_files
                .values()
                .all(|value| digest(value))
            && identity
                .package
                .loaded_extensions
                .values()
                .all(|value| digest(value))
            && !receipt.evidence.script.is_empty()
            && !receipt.evidence.executable.is_empty()
            && !receipt.evidence.resolved_executable.is_empty()
            && !receipt.evidence.package_root.is_empty()
            && !receipt.evidence.scope.is_empty()
            && identity
                .package
                .loaded_extensions
                .keys()
                .eq(receipt.evidence.extension_paths.keys()),
        "invalid vmprof decoder provenance; rebuild or reinstall matching decoder assets"
    );
    Ok(())
}

fn interpreter() -> PathBuf {
    if let Some(value) = env::var_os("HUNIN_VMPROF_PYTHON") {
        return PathBuf::from(value);
    }
    if let Some(data) = dirs::data_local_dir() {
        let path = data.join("hunin/tools/vmprof").join(if cfg!(windows) {
            "Scripts/python.exe"
        } else {
            "bin/python"
        });
        if path.is_file() {
            return path;
        }
    }
    tool("python3")
        .or_else(|| tool("python"))
        .unwrap_or_else(|| PathBuf::from("python3"))
}

pub(super) struct Decoded {
    raw: Value,
    pub provenance: Value,
}

pub(super) fn read(path: &Path) -> Result<Decoded> {
    let python = interpreter();
    let root = assets();
    let source = root.join("src/importers/vmprof/decode.py");
    let script = if source.is_file() {
        source
    } else {
        root.join("collectors/python/decode.py")
    };
    let script_sha256 = format!("{:x}", Sha256::digest(std::fs::read(&script)?));
    let output = probe::output(
        Command::new(&python).arg(&script).arg(path),
        None,
        std::time::Duration::from_secs(60),
        64 * 1024 * 1024,
    )
    .with_context(|| format!("run vmprof decoder with {}", python.display()))?;
    if !output.status.success() {
        bail!(
            "vmprof decoding failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut raw: Value = serde_json::from_slice(&output.stdout)?;
    let provenance = raw
        .as_object_mut()
        .and_then(|fields| fields.remove("decoder_provenance"))
        .context(
            "vmprof decoder provenance missing; rebuild or reinstall matching Hunin decoder assets",
        )?;
    validate_provenance(&provenance)?;
    let actual = format!("{:x}", Sha256::digest(std::fs::read(&script)?));
    anyhow::ensure!(
        provenance["identity"]["script_sha256"] == actual && actual == script_sha256,
        "vmprof decoder source changed during normalization"
    );
    Ok(Decoded { raw, provenance })
}

pub(super) fn decode(
    decoded: &Decoded,
    run_id: &str,
    key: &str,
) -> Result<(Vec<Value>, Vec<String>)> {
    let raw = &decoded.raw;
    let pid = raw["pid"].as_u64().unwrap_or(0);
    let clock = raw["clock"].as_str().unwrap_or("unknown");
    let mut records = Vec::new();
    for (index, value) in raw["samples"]
        .as_array()
        .context("vmprof samples missing")?
        .iter()
        .enumerate()
    {
        let mut record = sample(
            run_id,
            key,
            index,
            StackSample {
                pid,
                tid: value["tid"].as_str().unwrap_or("unknown"),
                timestamp_us: value["timestamp_us"].as_u64().unwrap_or(0),
                stack: value["stack"]
                    .as_array()
                    .context("vmprof stack missing")?
                    .clone(),
                weight: 1.0,
                weight_unit: SampleWeightUnit::Samples,
                attributes: json!({"clock":clock,"clock_alignment":"unmapped","timestamp_available":value["timestamp_available"],
                "estimated_timer_periods":value["estimated_timer_periods"],"source_format":"vmprof",
                "sampling_basis":raw["sampling_basis"],"profile_os":raw["profile_os"],
                "profile_metadata":raw["metadata"],"lost_time_seconds":raw["lost_time_seconds"]}),
            },
        );
        record["interval_us"] = raw["interval_us"].clone();
        records.push(record);
        if let Some(bytes) = value["rss_bytes"].as_u64() {
            records.push(json!({"schema_version":1,"kind":"metric","id":format!("profile:{key}:rss:{index}"),"run_id":run_id,
                "producer_id":format!("profile:{key}:{pid}"),"pid":pid,"tid":"","timestamp_us":value["timestamp_us"],
                "basis":"observed","name":"rss","value":bytes,"unit":"bytes","scope":"process",
                "attributes":{"clock":clock,"clock_alignment":"unmapped","source_format":"vmprof"}}));
        }
    }
    Ok((
        records,
        vec![format!(
            "vmprof timestamps use an unaligned {clock} clock; inspect hotspots separately from the epoch timeline"
        )],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identified_decoder_requires_runtime_package_and_options_evidence() {
        let mut receipt = json!({
            "schema_version":1,
            "identity":{
                "decoder":"hunin-vmprof-v1","script_sha256":"a".repeat(64),
                "runtime":{"implementation":"cpython","version":[3,12,10,"final",0],"executable_sha256":"b".repeat(64),"byteorder":"little","default_text_encoding":"UTF-8","flags":{"optimize":0}},
                "package":{"name":"vmprof","version":"0.6.0","python_files":{"reader.py":"c".repeat(64)},"loaded_extensions":{}}
            },
            "evidence":{"script":"/decoder.py","executable":"/python","resolved_executable":"/python","package_root":"/vmprof","extension_paths":{},"scope":"test"}
        });
        validate_provenance(&receipt).unwrap();
        for field in ["runtime", "package"] {
            let mut invalid = receipt.clone();
            invalid["identity"][field] = json!({});
            assert!(validate_provenance(&invalid).is_err());
        }
        receipt["identity"]["runtime"]["flags"] = json!({});
        assert!(validate_provenance(&receipt).is_err());
    }
}

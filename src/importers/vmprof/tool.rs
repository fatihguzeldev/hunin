//! Execute the offline decoder and validate its observed source/runtime/package identity.
use crate::host::{assets, probe, tool};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
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

pub(in crate::importers) struct Decoded {
    pub(super) raw: Value,
    pub provenance: Value,
}

pub(in crate::importers) fn read(path: &Path, timeout: std::time::Duration) -> Result<Decoded> {
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
        timeout,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path, process::Command, time::Duration};

pub(crate) const VERSION: &str = "samply 0.13.1-hunin-native1";

pub(crate) fn check(path: &Path, reported: Option<&str>, timeout: Duration) -> Result<String> {
    let version = match reported {
        Some(version) => version.to_owned(),
        None => output(Command::new(path).arg("--version"), timeout)?,
    };
    ensure!(
        version == VERSION,
        "native module lifetime extension missing or incompatible: requires {VERSION}, found {version:?}. Use Hunin's bundled collector or set HUNIN_SAMPLY to a patched build"
    );
    let help = output(Command::new(path).args(["record", "--help"]), timeout)?;
    ensure!(
        help.contains("--lifetime-output") && help.contains("--unstable-presymbolicate"),
        "selected sampler does not expose Hunin's required lifetime-output and presymbolication flags; reinstall the bundled native collector"
    );
    #[cfg(windows)]
    {
        let helper = output(
            Command::new(path).args(["run-elevated-helper", "--help"]),
            timeout,
        )?;
        ensure!(
            helper.contains("--parent-pid") && helper.contains("--parent-creation-100ns"),
            "selected native collector lacks the required Windows wrapper lifetime cleanup protocol; reinstall Hunin's bundled collector"
        );
    }
    let bundled = super::assets()
        .join("collectors/native/bin")
        .join(if cfg!(windows) {
            "samply.exe"
        } else {
            "samply"
        });
    let managed = path.canonicalize().ok() == bundled.canonicalize().ok() && bundled.is_file();
    let _ = verify_receipt(path, managed)?;
    Ok(version)
}

pub(crate) fn identity(binary: &Path, version: &str) -> Result<Value> {
    let receipt = verify_receipt(binary, false)?;
    let sha256 = match &receipt {
        Some(receipt) => receipt["binary_sha256"].as_str().unwrap().to_owned(),
        None => binary_sha256(binary)?,
    };
    Ok(
        serde_json::json!({"name":"samply-hunin","version":version,"binary_sha256":sha256,
        "provenance":receipt,"identity_scope":"selected collector executable bytes and any verified build receipt; distinct from target runtime version"}),
    )
}

fn output(command: &mut Command, timeout: Duration) -> Result<String> {
    let output = super::probe::output(command, None, timeout, 32 * 1024)?;
    ensure!(
        output.status.success(),
        "native collector prerequisite probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn verify_receipt(binary: &Path, required: bool) -> Result<Option<Value>> {
    let name = binary
        .file_name()
        .context("native collector filename missing")?;
    let receipt = binary.with_file_name(format!("{}.provenance.json", name.to_string_lossy()));
    if !required && !receipt.exists() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    fs::File::open(&receipt)
        .with_context(|| format!("native collector provenance missing: {}", receipt.display()))?
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 256 * 1024,
        "native collector provenance exceeds 256 KiB"
    );
    let receipt: Value =
        serde_json::from_slice(&bytes).context("invalid native collector provenance")?;
    let manifest: Value =
        serde_json::from_str(include_str!("../../collectors/native/manifest.json"))?;
    ensure!(
        receipt["schema_version"] == 1
            && receipt["version"] == manifest["version"]
            && receipt["identity"]["upstream"] == manifest["upstream"]
            && receipt["extension"] == manifest["extension"],
        "native collector provenance does not match the pinned Hunin collector"
    );
    let expected = receipt["binary_sha256"]
        .as_str()
        .context("native collector binary SHA256 missing")?;
    ensure!(
        binary_sha256(binary)? == expected,
        "native collector executable does not match its provenance SHA256"
    );
    Ok(Some(receipt))
}

fn binary_sha256(binary: &Path) -> Result<String> {
    let mut source = fs::File::open(binary)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Mirrors the pinned collector's discovery exactly; no unsupported override.
#[cfg(windows)]
pub(crate) fn xperf_path() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|directory| directory.join("xperf.exe"))
            .find(|candidate| candidate.is_file())
    });
    path.or_else(|| {
        std::env::var_os("ProgramFiles(x86)")
            .map(std::path::PathBuf::from)
            .map(|directory| {
                directory.join("Windows Kits/10/Windows Performance Toolkit/xperf.exe")
            })
            .filter(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn managed_collector_requires_its_pinned_receipt_and_unmodified_binary() {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("samply");
        fs::write(&binary, b"collector executable").unwrap();
        assert!(verify_receipt(&binary, true).is_err());
        assert!(verify_receipt(&binary, false).is_ok());
        let manifest: Value =
            serde_json::from_str(include_str!("../../collectors/native/manifest.json")).unwrap();
        let mut receipt = json!({"schema_version":1,"version":manifest["version"],"identity":{"upstream":manifest["upstream"]},"extension":manifest["extension"],"binary_sha256":format!("{:x}",Sha256::digest(b"collector executable"))});
        let path = root.path().join("samply.provenance.json");
        fs::write(&path, receipt.to_string()).unwrap();
        let _ = verify_receipt(&binary, true).unwrap();
        let before = identity(&binary, VERSION).unwrap();
        fs::write(&binary, b"modified executable").unwrap();
        assert!(
            verify_receipt(&binary, true)
                .unwrap_err()
                .to_string()
                .contains("SHA256")
        );
        fs::remove_file(&path).unwrap();
        let changed = identity(&binary, VERSION).unwrap();
        assert_ne!(before["binary_sha256"], changed["binary_sha256"]);
        assert_eq!(before["version"], changed["version"]);
        receipt["version"] = json!("0.13.1");
        fs::write(path, receipt.to_string()).unwrap();
        assert!(
            verify_receipt(&binary, true)
                .unwrap_err()
                .to_string()
                .contains("pinned")
        );
    }
}

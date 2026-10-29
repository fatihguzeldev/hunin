//! Keep the launched target's wait status separate from the profiler process.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufReader, Read},
    path::Path,
};

#[derive(Deserialize)]
struct Envelope {
    schema_version: u32,
    raw_profile_sha256: String,
    collector: Collector,
    target_outcome: Value,
}

#[derive(Deserialize)]
struct Collector {
    revision: String,
    binary_sha256: String,
}

fn hash_file(path: &Path) -> Result<String> {
    let mut input = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn read(directory: &Path, collector: &Path) -> Result<Value> {
    let sidecar = directory.join("cpu.samply.lifetimes.json");
    ensure!(
        fs::metadata(&sidecar)?.len() <= 160 * 1024 * 1024,
        "native outcome sidecar exceeds160MiB read budget"
    );
    // Deserialize ignored event arrays without allocating the whole lifetime
    // trace. Only the small outcome and producer envelope are retained.
    let envelope: Envelope = serde_json::from_reader(BufReader::new(fs::File::open(&sidecar)?))
        .context("native collector did not provide a readable root target outcome")?;
    ensure!(
        envelope.schema_version == 1 && envelope.collector.revision == "hunin-native1",
        "unsupported native outcome producer"
    );
    ensure!(
        envelope.raw_profile_sha256 == hash_file(&directory.join("cpu.samply.json"))?,
        "native target outcome does not match its raw profile"
    );
    ensure!(
        envelope.collector.binary_sha256 == hash_file(collector)?,
        "native target outcome does not match the supervised collector"
    );
    validate(&envelope.target_outcome)?;
    Ok(envelope.target_outcome)
}

pub(super) fn validate(outcome: &Value) -> Result<()> {
    ensure!(
        outcome["source"] == "root_target_os_wait_status",
        "native target outcome has no OS wait-status provenance"
    );
    ensure!(
        outcome["pid"]
            .as_u64()
            .is_some_and(|pid| pid > 0 && pid <= u32::MAX as u64),
        "native target PID missing"
    );
    let started = outcome["launch_observed_at_us"]
        .as_u64()
        .context("native target launch observation missing")?;
    match outcome["state"].as_str() {
        Some("exited") => {
            ensure!(
                outcome["exit_code"]
                    .as_i64()
                    .is_some_and(|code| i32::try_from(code).is_ok())
                    && outcome["signal"].is_null(),
                "invalid native target exit status"
            );
        }
        Some("signaled") => {
            ensure!(
                outcome["exit_code"].is_null()
                    && outcome["signal"]
                        .as_u64()
                        .is_some_and(|signal| signal > 0 && signal <= i32::MAX as u64),
                "invalid native target signal status"
            );
        }
        Some("running" | "unknown") => {
            ensure!(
                outcome["exit_code"].is_null(),
                "unknown native target outcome cannot provide an exit code"
            );
            return Ok(());
        }
        _ => anyhow::bail!("native target outcome state missing or unsupported"),
    }
    ensure!(
        outcome["wait_observed_at_us"]
            .as_u64()
            .is_some_and(|ended| ended >= started),
        "native target wait observation missing or precedes launch"
    );
    Ok(())
}

pub(crate) fn unknown(reason: &str) -> Value {
    json!({"state":"unknown","exit_code":null,"signal":null,"reason":reason,"source":"native target status unavailable; collector exit is a separate observation"})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_exit_is_bound_to_both_raw_profile_and_collector() {
        let root = tempfile::tempdir().unwrap();
        let collector = root.path().join("collector");
        fs::write(&collector, b"collector").unwrap();
        fs::write(root.path().join("cpu.samply.json"), b"profile").unwrap();
        let outcome = json!({"state":"exited","pid":8,"exit_code":2,"signal":null,"launch_observed_at_us":10,"wait_observed_at_us":20,"source":"root_target_os_wait_status"});
        let envelope = json!({"schema_version":1,"raw_profile_sha256":hash_file(&root.path().join("cpu.samply.json")).unwrap(),"collector":{"revision":"hunin-native1","binary_sha256":hash_file(&collector).unwrap()},"target_outcome":outcome,"events":[{"kind":"ignored"}]});
        fs::write(
            root.path().join("cpu.samply.lifetimes.json"),
            envelope.to_string(),
        )
        .unwrap();
        assert_eq!(read(root.path(), &collector).unwrap()["exit_code"], 2);
        fs::write(&collector, b"replaced").unwrap();
        assert!(
            read(root.path(), &collector)
                .unwrap_err()
                .to_string()
                .contains("supervised collector")
        );
        let mut signaled = outcome.clone();
        signaled["state"] = json!("signaled");
        signaled["exit_code"] = Value::Null;
        signaled["signal"] = json!(11);
        validate(&signaled).unwrap();
        signaled["exit_code"] = json!(0);
        assert!(validate(&signaled).is_err());
    }
}

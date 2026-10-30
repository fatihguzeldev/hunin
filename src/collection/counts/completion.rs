use crate::collection::CaptureRequest;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

fn bounded(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("read count collector output {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(maximum + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= maximum,
        "function-count output exceeds {maximum} bytes"
    );
    Ok(bytes)
}

pub(in crate::collection) fn validate(
    request: &CaptureRequest,
    files: &[PathBuf],
    method: &str,
    maximum: u64,
) -> Result<()> {
    let profile = request.artifact_dir.join("calls.calls.jsonl");
    let completion = request.artifact_dir.join("calls.complete.json");
    ensure!(
        files.contains(&profile) && files.contains(&completion),
        "function-count collector did not finalize its declared profile and completion manifest"
    );
    let report: Value = serde_json::from_slice(&bounded(&completion, 64 * 1024)?)?;
    ensure!(
        report["schema_version"] == 1
            && report["run_id"] == request.run_id
            && report["method"] == method,
        "function-count completion identity does not match this recording"
    );
    ensure!(
        report["status"] == "complete",
        "function-count collection incomplete: {}",
        report
    );
    ensure!(
        report["dropped_events"].as_u64() == Some(0)
            && report["counter_overflow"] == false
            && report["output_truncated"] == false
            && report["loss"] == "none_observed",
        "function-count completion contradicts its loss or bounds report"
    );
    if method == "cpython-monitoring-native" {
        ensure!(
            report["observer_intact"] == true,
            "Python count observer ownership was not intact at finalization"
        );
    }
    let bytes = bounded(&profile, maximum)?;
    ensure!(
        report["bytes"].as_u64() == Some(bytes.len() as u64),
        "function-count output length does not match completion manifest"
    );
    if let Some(digest) = report["sha256"].as_str() {
        ensure!(
            digest == format!("{:x}", Sha256::digest(&bytes)),
            "function-count output digest does not match completion manifest"
        );
    } else {
        let digest = bytes.iter().fold(0xcbf29ce484222325u64, |value, byte| {
            (value ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
        ensure!(
            report["fnv1a64"].as_str() == Some(format!("{digest:016x}").as_str()),
            "function-count output integrity does not match completion manifest"
        );
    }
    let mut records = 0u64;
    let mut complete = false;
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let record: Value = serde_json::from_slice(line)?;
        ensure!(
            record["run_id"] == request.run_id,
            "function count belongs to a different recording"
        );
        ensure!(
            record["producer_id"] == format!("calls:{}", request.run_id),
            "function-count producer identity does not match this recording"
        );
        if record["kind"] == "event"
            && record["name"] == "function.counts.complete"
            && record["attributes"]["status"] == "complete"
            && record["attributes"]["method"] == method
        {
            complete = true;
        }
        records += 1;
    }
    ensure!(
        report["records"].as_u64() == Some(records),
        "function-count record inventory differs from completion manifest"
    );
    ensure!(
        complete,
        "function-count profile has no matching accepted completion event"
    );
    Ok(())
}

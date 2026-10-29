use super::sample;
use crate::{
    host::{assets, probe, tool},
    model::observations::StackSample,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

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

pub(super) fn decode(path: &Path, run_id: &str, key: &str) -> Result<(Vec<Value>, Vec<String>)> {
    let python = interpreter();
    let output = probe::output(
        Command::new(&python)
            .arg(assets().join("collectors/python/decode.py"))
            .arg(path),
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
    let raw: Value = serde_json::from_slice(&output.stdout)?;
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
                weight_unit: "samples",
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

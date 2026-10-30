//! Translate decoded vmprof samples while retaining CPU-clock uncertainty.
use super::tool::Decoded;
use crate::{
    importers::output::{RecordBatch, sample},
    model::observations::{SampleWeightUnit, StackSample},
};
use anyhow::{Context, Result};
use serde_json::json;

pub(in crate::importers) fn decode_into(
    decoded: &Decoded,
    run_id: &str,
    key: &str,
    records: &mut RecordBatch<'_>,
) -> Result<Vec<String>> {
    let raw = &decoded.raw;
    let pid = raw["pid"].as_u64().unwrap_or(0);
    let clock = raw["clock"].as_str().unwrap_or("unknown");
    for name in [
        "metadata",
        "sampling_basis",
        "profile_os",
        "lost_time_seconds",
    ] {
        records.fragment(&raw[name])?;
    }
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
                stack: records
                    .clone_stack(value["stack"].as_array().context("vmprof stack missing")?)?,
                weight: 1.0,
                weight_unit: SampleWeightUnit::Samples,
                attributes: json!({"clock":clock,"clock_alignment":"unmapped","timestamp_available":value["timestamp_available"],
                "estimated_timer_periods":value["estimated_timer_periods"],"source_format":"vmprof",
                "sampling_basis":raw["sampling_basis"],"profile_os":raw["profile_os"],
                "profile_metadata":raw["metadata"],"lost_time_seconds":raw["lost_time_seconds"]}),
            },
        );
        record["interval_us"] = raw["interval_us"].clone();
        records.push(record)?;
        if let Some(bytes) = value["rss_bytes"].as_u64() {
            records.push(json!({"schema_version":1,"kind":"metric","id":format!("profile:{key}:rss:{index}"),"run_id":run_id,
                "producer_id":format!("profile:{key}:{pid}"),"pid":pid,"tid":"","timestamp_us":value["timestamp_us"],
                "basis":"observed","name":"rss","value":bytes,"unit":"bytes","scope":"process",
                "attributes":{"clock":clock,"clock_alignment":"unmapped","source_format":"vmprof"}}))?;
        }
    }
    Ok(vec![format!(
        "vmprof timestamps use an unaligned {clock} clock; inspect hotspots separately from the epoch timeline"
    )])
}

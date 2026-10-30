use super::WorkBudget;
use crate::model::numbers::unsigned_integer;
use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub fn resources(records: &[Value]) -> Value {
    resources_page(records, &json!({}))
}

pub fn resources_page(records: &[Value], params: &Value) -> Value {
    resources_page_checked(records, params, &mut WorkBudget::unlimited())
        .expect("unlimited query work")
}

pub(super) fn resources_page_checked(
    records: &[Value],
    params: &Value,
    work: &mut WorkBudget,
) -> Result<Value> {
    let mut groups: BTreeMap<String, (Value, Vec<&Value>)> = BTreeMap::new();
    let mut unmapped_points = 0;
    for record in records.iter().filter(|r| r["kind"] == "metric") {
        work.check()?;
        let unmapped = record["attributes"]["clock_alignment"] == "unmapped";
        let timestamp = unsigned_integer(&record["timestamp_us"]).unwrap_or(0);
        if !unmapped
            && (!params["start_us"]
                .as_u64()
                .is_none_or(|start| timestamp >= start)
                || !params["end_us"].as_u64().is_none_or(|end| timestamp <= end))
        {
            continue;
        }
        if unmapped {
            unmapped_points += 1;
        }
        let attrs = &record["attributes"];
        let clock =
            attrs["clock"]
                .as_str()
                .unwrap_or(if unmapped { "unknown" } else { "unix_epoch" });
        let mapping = attrs["clock_alignment"]
            .as_str()
            .unwrap_or("default_epoch_contract");
        let thread_source = attrs["thread_id_source"]
            .as_str()
            .or_else(|| attrs["tid_kind"].as_str())
            .unwrap_or("producer-local");
        let metadata = json!({"name":record["name"],"unit":record["unit"],"scope":record["scope"],"pid":record["pid"],"tid":record["tid"],"producer_id":record["producer_id"],"context_id":record["context_id"],"thread_id_source":thread_source,"clock":clock,"clock_alignment":if unmapped{"unmapped"}else{"aligned"},"time_mapping":mapping,"process_started_at_seconds":attrs["process_started_at_seconds"],"process_lifetime_id":attrs["process_lifetime_id"],"process_role":attrs["process_role"],"temporality":attrs["temporality"]});
        let key = metadata.to_string();
        if !groups.contains_key(&key) {
            work.entry()?;
        }
        groups
            .entry(key)
            .or_insert_with(|| (metadata, Vec::new()))
            .1
            .push(record);
    }
    for (_, points) in groups.values_mut() {
        work.check()?;
        points.sort_by(|a, b| {
            unsigned_integer(&a["timestamp_us"])
                .cmp(&unsigned_integer(&b["timestamp_us"]))
                .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
        });
        work.check()?;
    }
    let total_points = groups
        .values()
        .map(|(_, points)| points.len())
        .sum::<usize>();
    let offset = params["cursor"]
        .as_str()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let series_limit = params["limit"].as_u64().unwrap_or(100).clamp(1, 5000) as usize;
    let point_limit = params["point_limit"]
        .as_u64()
        .unwrap_or(1000)
        .clamp(1, 5000) as usize;
    let mut skip = offset;
    let mut returned = 0;
    let mut series = Vec::new();
    for (key, (mut metadata, points)) in groups {
        work.check()?;
        if skip >= points.len() {
            skip -= points.len();
            continue;
        }
        if series.len() >= series_limit || returned >= point_limit {
            break;
        }
        let count = (points.len() - skip).min(point_limit - returned);
        metadata["id"] = json!(format!("resource:{:x}", Sha256::digest(key.as_bytes())));
        metadata["point_count"] = json!(points.len());
        metadata["point_start"] = json!(skip);
        metadata["returned_points"] = json!(count);
        metadata["truncated"] = json!(skip > 0 || count < points.len());
        metadata["points"]=json!(points.iter().skip(skip).take(count).map(|r|json!({"id":r["id"],"timestamp_us":r["timestamp_us"],"value":r["value"],"pid":r["pid"],"tid":r["tid"],"context_id":r["context_id"],"producer_id":r["producer_id"]})).collect::<Vec<_>>());
        series.push(metadata);
        returned += count;
        skip = 0;
    }
    let next = offset.saturating_add(returned);
    let diagnostics = if unmapped_points > 0
        && (params.get("start_us").is_some() || params.get("end_us").is_some())
    {
        vec![
            "Epoch windows do not filter unmapped resource clocks; inspect those series in their declared clock domain.",
        ]
    } else {
        Vec::new()
    };
    work.check()?;
    Ok(
        json!({"series":series,"next_cursor":if next<total_points{Some(next.to_string())}else{None},"returned_points":returned,"total_points":total_points,"point_limit":point_limit,"unaligned_point_count":unmapped_points,"diagnostics":diagnostics}),
    )
}

use serde_json::{Value, json};
use std::collections::BTreeSet;

pub fn timeline(records: &[Value], params: &Value) -> Value {
    let alignment = params["alignment"].as_str().unwrap_or("aligned");
    let relevant = records
        .iter()
        .filter(|r| {
            params["kind"].as_str().is_none_or(|kind| r["kind"] == kind)
                && params["pid"]
                    .as_u64()
                    .is_none_or(|pid| r["pid"].as_u64() == Some(pid))
        })
        .collect::<Vec<_>>();
    let is_unmapped = |r: &Value| r["attributes"]["clock_alignment"] == "unmapped";
    let unaligned = relevant.iter().filter(|r| is_unmapped(r)).count();
    let aligned = relevant
        .iter()
        .copied()
        .filter(|r| !is_unmapped(r))
        .collect::<Vec<_>>();
    let bounds = if alignment == "unmapped" {
        Value::Null
    } else {
        let start = aligned
            .iter()
            .filter_map(|r| r["timestamp_us"].as_u64())
            .min()
            .unwrap_or(0);
        let end = aligned
            .iter()
            .filter_map(|r| {
                r["timestamp_us"]
                    .as_u64()
                    .map(|ts| ts.saturating_add(r["duration_us"].as_u64().unwrap_or(0)))
            })
            .max()
            .unwrap_or(start);
        json!({"start_us":start,"end_us":end})
    };
    let selected = relevant
        .into_iter()
        .filter(|r| {
            if is_unmapped(r) {
                return alignment == "unmapped" || alignment == "all";
            }
            if alignment == "unmapped" {
                return false;
            }
            let start = r["timestamp_us"].as_u64().unwrap_or(0);
            let end = start.saturating_add(r["duration_us"].as_u64().unwrap_or(0));
            params["start_us"].as_u64().is_none_or(|n| end >= n)
                && params["end_us"].as_u64().is_none_or(|n| start <= n)
        })
        .collect::<Vec<_>>();
    let offset = params["cursor"]
        .as_str()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = params["limit"].as_u64().unwrap_or(500).clamp(1, 5000) as usize;
    let page = selected
        .iter()
        .copied()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let next = offset.saturating_add(page.len());
    let (rows, unaligned_rows): (Vec<_>, Vec<_>) = if alignment == "all" {
        page.into_iter().partition(|r| !is_unmapped(r))
    } else {
        (page, Vec::new())
    };
    let clock_domains = selected
        .iter()
        .filter(|r| is_unmapped(r))
        .map(|r| {
            json!({"producer_id":r["producer_id"],"clock":r["attributes"]["clock"]}).to_string()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|v| serde_json::from_str::<Value>(&v).unwrap())
        .collect::<Vec<_>>();
    json!({"records":rows,"unaligned_records":unaligned_rows,"alignment":alignment,"next_cursor":if next<selected.len(){Some(next.to_string())}else{None},"bounds":bounds,"unaligned_count":unaligned,"unaligned_clock_domains":clock_domains,"total_count":selected.len(),"returned_count":rows.len()+unaligned_rows.len()})
}

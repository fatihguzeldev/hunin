use super::WorkBudget;
use crate::model::numbers::unsigned_integer;
use crate::storage::QueryReadPort;
use anyhow::Result;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Keep scalar fields as JSON values: historical rows may carry PIDs outside
/// u64, and the slice API deliberately treats those as unavailable for filters.
#[derive(Deserialize)]
struct Projection {
    #[serde(default)]
    kind: Value,
    #[serde(default)]
    pid: Value,
    #[serde(default)]
    timestamp_us: Value,
    #[serde(default)]
    duration_us: Value,
    #[serde(default)]
    producer_id: Value,
    #[serde(default)]
    attributes: Value,
}

pub(super) fn read(
    port: &mut QueryReadPort<'_>,
    params: &Value,
    work: &mut WorkBudget,
) -> Result<Value> {
    let alignment = params["alignment"].as_str().unwrap_or("aligned");
    let offset = params["cursor"]
        .as_str()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = params["limit"].as_u64().unwrap_or(500).clamp(1, 5000) as usize;
    let mut start: Option<u64> = None;
    let mut end: Option<u64> = None;
    let mut unaligned = 0usize;
    let mut total = 0usize;
    let mut page = Vec::new();
    let mut unaligned_page = Vec::new();
    let mut domains = BTreeSet::new();
    let kind = params["kind"].as_str();
    let kinds = kind.into_iter().collect::<Vec<_>>();
    port.visit(&kinds, |text| {
        work.check()?;
        let row: Projection = serde_json::from_str(text)?;
        if kind.is_some_and(|kind| row.kind != kind)
            || params["pid"]
                .as_u64()
                .is_some_and(|pid| unsigned_integer(&row.pid) != Some(pid))
        {
            return Ok(());
        }
        let unmapped = row.attributes["clock_alignment"] == "unmapped";
        if unmapped {
            unaligned += 1;
        } else if let Some(timestamp) = unsigned_integer(&row.timestamp_us) {
            let until = timestamp.saturating_add(unsigned_integer(&row.duration_us).unwrap_or(0));
            start = Some(start.map_or(timestamp, |old| old.min(timestamp)));
            end = Some(end.map_or(until, |old| old.max(until)));
        }
        let selected = if unmapped {
            alignment == "unmapped" || alignment == "all"
        } else if alignment == "unmapped" {
            false
        } else {
            let timestamp = unsigned_integer(&row.timestamp_us).unwrap_or(0);
            let until = timestamp.saturating_add(unsigned_integer(&row.duration_us).unwrap_or(0));
            params["start_us"]
                .as_u64()
                .is_none_or(|value| until >= value)
                && params["end_us"]
                    .as_u64()
                    .is_none_or(|value| timestamp <= value)
        };
        if !selected {
            return Ok(());
        }
        if unmapped {
            let domain =
                json!({"producer_id":row.producer_id,"clock":row.attributes["clock"]}).to_string();
            if !domains.contains(&domain) {
                work.entry()?;
                domains.insert(domain);
            }
        }
        if total >= offset && page.len() + unaligned_page.len() < limit {
            let value = serde_json::from_str::<Value>(text)?;
            if alignment == "all" && unmapped {
                unaligned_page.push(value);
            } else {
                page.push(value);
            }
        }
        total += 1;
        work.check()
    })?;
    let returned = page.len() + unaligned_page.len();
    let next = offset.saturating_add(returned);
    let bounds = if alignment == "unmapped" {
        Value::Null
    } else {
        json!({"start_us":start.unwrap_or(0),"end_us":end.unwrap_or(start.unwrap_or(0))})
    };
    let domains = domains
        .into_iter()
        .map(|text| serde_json::from_str::<Value>(&text))
        .collect::<serde_json::Result<Vec<_>>>()?;
    Ok(
        json!({"records":page,"unaligned_records":unaligned_page,"alignment":alignment,"next_cursor":if next<total{Some(next.to_string())}else{None},"bounds":bounds,"unaligned_count":unaligned,"unaligned_clock_domains":domains,"total_count":total,"returned_count":returned}),
    )
}

pub fn timeline(records: &[Value], params: &Value) -> Value {
    let alignment = params["alignment"].as_str().unwrap_or("aligned");
    let relevant = records
        .iter()
        .filter(|r| {
            params["kind"].as_str().is_none_or(|kind| r["kind"] == kind)
                && params["pid"]
                    .as_u64()
                    .is_none_or(|pid| unsigned_integer(&r["pid"]) == Some(pid))
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
            .filter_map(|r| unsigned_integer(&r["timestamp_us"]))
            .min()
            .unwrap_or(0);
        let end = aligned
            .iter()
            .filter_map(|r| {
                unsigned_integer(&r["timestamp_us"])
                    .map(|ts| ts.saturating_add(unsigned_integer(&r["duration_us"]).unwrap_or(0)))
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
            let start = unsigned_integer(&r["timestamp_us"]).unwrap_or(0);
            let end = start.saturating_add(unsigned_integer(&r["duration_us"]).unwrap_or(0));
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

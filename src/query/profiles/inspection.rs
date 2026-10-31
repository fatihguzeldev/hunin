//! Coordinated profile views share a bounded selected-observation snapshot.
use super::{HotspotAccumulator, TreeAccumulator, number, tree::frame_id};
use crate::{model::numbers::unsigned_integer, query::WorkBudget, storage::QueryReadPort};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};

struct Sample {
    timestamp: u64,
    record: Value,
}

struct Caller {
    frame: Value,
    weight: f64,
    count: usize,
}

#[derive(Default)]
struct Selected {
    frame: Option<Value>,
    first: u64,
    last: u64,
    own: f64,
    inclusive: f64,
    count: usize,
    evidence: Vec<String>,
    callers: BTreeMap<String, Caller>,
}

impl Selected {
    fn observe(
        &mut self,
        sample: &Sample,
        selected: &str,
        inside: bool,
        work: &mut WorkBudget,
    ) -> Result<()> {
        let Some(stack) = sample.record["stack"].as_array() else {
            return Ok(());
        };
        let mut found = false;
        let mut caller_ids = HashSet::new();
        let weight = number(&sample.record, "weight");
        for (index, frame) in stack.iter().enumerate() {
            work.frame()?;
            if frame_id(frame) != selected {
                continue;
            }
            if self.frame.is_none() {
                self.frame = Some(frame.clone());
                self.first = sample.timestamp;
                self.last = sample.timestamp;
            }
            self.first = self.first.min(sample.timestamp);
            self.last = self.last.max(sample.timestamp);
            found = true;
            if inside && index + 1 == stack.len() {
                self.own += weight;
            }
            if inside && index > 0 {
                let caller = &stack[index - 1];
                let id = frame_id(caller);
                if caller_ids.insert(id.clone()) {
                    if !self.callers.contains_key(&id) {
                        work.entry()?;
                    }
                    let row = self.callers.entry(id).or_insert_with(|| Caller {
                        frame: caller.clone(),
                        weight: 0.0,
                        count: 0,
                    });
                    row.weight += weight;
                    row.count += 1;
                }
            }
        }
        if found && inside {
            self.inclusive += weight;
            self.count += 1;
            if self.evidence.len() < 50
                && let Some(id) = sample.record["id"].as_str()
            {
                self.evidence.push(id.to_owned());
            }
        }
        Ok(())
    }

    fn finish(self, id: &str, unit: &str) -> Value {
        let Some(frame) = self.frame else {
            return Value::Null;
        };
        let mut result = function_row(&frame, id, unit, self.own, self.inclusive);
        let mut callers = self.callers.into_iter().collect::<Vec<_>>();
        callers.sort_by(|a, b| {
            b.1.weight
                .total_cmp(&a.1.weight)
                .then_with(|| a.0.cmp(&b.0))
        });
        result["callers_truncated"] = json!(callers.len() > 100);
        callers.truncate(100);
        result["callers"] = json!(callers.into_iter().map(|(id,row)|json!({"id":id,"function":row.frame["function"],"file":row.frame["file"],"line":row.frame["line"],"module":row.frame["module"],"code_id":row.frame["code_id"],"weight":row.weight,"sample_count":row.count})).collect::<Vec<_>>());
        result["sample_count"] = json!(self.count);
        result["observed_start_us"] = json!(self.first);
        result["observed_end_us"] = json!(self.last);
        result["evidence_count"] = json!(self.count);
        result["evidence_truncated"] = json!(self.evidence.len() < self.count);
        result["evidence_ids"] = json!(self.evidence);
        result
    }
}

fn function_row(frame: &Value, id: &str, unit: &str, own: f64, inclusive: f64) -> Value {
    json!({"id":id,"function":frame["function"],"file":frame["file"],"line":frame["line"],"module":frame["module"],"code_id":frame["code_id"],"source_position_basis":"first observed sampled position","self_weight":own,"inclusive_weight":inclusive,"weight_unit":unit})
}

fn bins(
    samples: &[Sample],
    start: u64,
    end: u64,
    count: usize,
    selected: &str,
    work: &mut WorkBudget,
) -> Result<Value> {
    struct Bin {
        start: u64,
        end: u64,
        weight: f64,
        selected: f64,
        count: usize,
    }
    let span = end - start;
    let count = count.min(span.max(1) as usize);
    let mut rows = (0..count)
        .map(|index| {
            let edge = |n| start + (span as u128 * n as u128).div_ceil(count as u128) as u64;
            Bin {
                start: edge(index),
                end: edge(index + 1),
                weight: 0.0,
                selected: 0.0,
                count: 0,
            }
        })
        .collect::<Vec<_>>();
    for sample in samples {
        work.check()?;
        if sample.timestamp < start || sample.timestamp > end {
            continue;
        }
        let index = if span == 0 {
            0
        } else {
            ((sample.timestamp - start) as u128 * count as u128 / span as u128) as usize
        }
        .min(count - 1);
        let row = &mut rows[index];
        let weight = number(&sample.record, "weight");
        row.weight += weight;
        row.count += 1;
        if !selected.is_empty()
            && let Some(leaf) = sample.record["stack"]
                .as_array()
                .and_then(|stack| stack.last())
        {
            work.frame()?;
            if frame_id(leaf) == selected {
                row.selected += weight;
            }
        }
    }
    Ok(json!(rows.into_iter().map(|row|json!({"start_us":row.start,"end_us":row.end,"weight":row.weight,"selected_weight":row.selected,"sample_count":row.count})).collect::<Vec<_>>()))
}

fn name_paths(nodes: &mut [Value], path: &mut Vec<String>, work: &mut WorkBudget) -> Result<bool> {
    let mut truncated = false;
    for node in nodes {
        work.frame()?;
        let frame = node["id"].as_str().unwrap_or("").to_owned();
        path.push(frame.clone());
        node["frame_id"] = json!(frame);
        node["id"] = json!(format!(
            "node:{:x}",
            Sha256::digest(serde_json::to_vec(path)?)
        ));
        truncated |= node["children_truncated"] == true || node["depth_truncated"] == true;
        if let Some(children) = node["children"].as_array_mut() {
            truncated |= name_paths(children, path, work)?;
        }
        path.pop();
    }
    Ok(truncated)
}

pub(in crate::query) fn inspect(
    port: &mut QueryReadPort<'_>,
    params: &Value,
    work: &mut WorkBudget,
) -> Result<Value> {
    let clock_domain = params["clock_domain"].as_str();
    let placed = |record: &Value| match clock_domain {
        Some(domain) => {
            crate::model::clocks::runtime_relative_domain(&record["attributes"]) == Some(domain)
        }
        None => crate::model::clocks::inspect(&record["attributes"]).aligned,
    };
    let mut records = Vec::new();
    let mut units = BTreeSet::new();
    let mut aligned_units = BTreeSet::new();
    port.visit(&["sample"], |text| {
        work.check()?;
        let record: Value = serde_json::from_str(text)?;
        let unit = record["weight_unit"]
            .as_str()
            .unwrap_or("samples")
            .to_owned();
        if units.insert(unit.clone()) {
            work.entry()?;
        }
        if placed(&record) && unsigned_integer(&record["timestamp_us"]).is_some() {
            aligned_units.insert(unit);
        }
        records.push(record);
        Ok(())
    })?;
    let candidates = if aligned_units.is_empty() {
        &units
    } else {
        &aligned_units
    };
    let unit = params["weight_unit"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if candidates.contains("samples") {
                "samples".into()
            } else {
                candidates
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "samples".into())
            }
        });
    let mut unaligned = 0;
    let mut samples = Vec::new();
    let clock_domains = records
        .iter()
        .filter(|record| record["weight_unit"] == unit)
        .filter_map(|record| crate::model::clocks::runtime_relative_domain(&record["attributes"]))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    for record in records {
        work.check()?;
        if record["weight_unit"] != unit {
            continue;
        }
        if !placed(&record) {
            unaligned += 1;
            continue;
        }
        let Some(timestamp) = unsigned_integer(&record["timestamp_us"]) else {
            unaligned += 1;
            continue;
        };
        samples.push(Sample { timestamp, record });
    }
    let bounds = samples
        .iter()
        .map(|s| s.timestamp)
        .min()
        .zip(samples.iter().map(|s| s.timestamp).max())
        .map(|(start, end)| {
            (
                start,
                if start == end {
                    end.saturating_add(1).min(9_007_199_254_740_991)
                } else {
                    end
                },
            )
        });
    let window = bounds.map(|(start, end)| {
        (
            params["start_us"].as_u64().unwrap_or(start),
            params["end_us"].as_u64().unwrap_or(end),
        )
    });
    if let Some((start, end)) = window {
        ensure!(start <= end, "profile window start must not exceed end");
    }
    let selected_id = params["frame_id"].as_str().unwrap_or("");
    let mut selected = Selected::default();
    let mut hotspots = HotspotAccumulator::profile(&unit);
    let mut tree_params = json!({"weight_unit":unit,"limit":params["node_limit"].as_u64().unwrap_or(2000),"depth_limit":params["depth_limit"].as_u64().unwrap_or(64)});
    tree_params["path"] = json!([]);
    let mut tree = TreeAccumulator::new(&tree_params);
    let mut count = 0usize;
    let mut capture_weight = 0.0;
    let mut total_weight = 0.0;
    for sample in &samples {
        work.check()?;
        let weight = number(&sample.record, "weight");
        capture_weight += weight;
        let inside =
            window.is_some_and(|(start, end)| sample.timestamp >= start && sample.timestamp <= end);
        if !selected_id.is_empty() {
            selected.observe(sample, selected_id, inside, work)?;
        }
        if inside {
            count += 1;
            total_weight += weight;
            hotspots.add(&sample.record, work)?;
            tree.add(&sample.record, work)?;
        }
    }
    let page = hotspots.finish(
        params["function_limit"].as_u64().unwrap_or(200) as usize,
        work,
    )?;
    let mut functions = page.result["hotspots"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for row in &mut functions {
        work.check()?;
        row["id"] = json!(frame_id(row));
    }
    let mut tree = tree.finish(&tree_params, work)?;
    let mut roots = tree["roots"].take();
    let tree_truncated = name_paths(roots.as_array_mut().unwrap(), &mut vec![], work)?
        || !tree["next_cursor"].is_null();
    let requested_bins = params["bins"].as_u64().unwrap_or(120).clamp(1, 240) as usize;
    let overview = if let Some((start, end)) = bounds {
        bins(&samples, start, end, requested_bins, selected_id, work)?
    } else {
        json!([])
    };
    let activity = if let Some((start, end)) = window {
        bins(&samples, start, end, requested_bins, selected_id, work)?
    } else {
        json!([])
    };
    let bounds_value = |range: Option<(u64, u64)>| {
        range.map(|(start, end)| json!({"start_us":start,"end_us":end}))
    };
    let mut diagnostics = vec![];
    if unaligned > 0 {
        diagnostics.push(format!("{unaligned} samples have no aligned timestamp."));
    }
    if samples.is_empty() {
        diagnostics.push("No aligned samples are available for this weight unit.".into());
    }
    if samples
        .iter()
        .any(|sample| sample.record["stack"].as_array().is_none_or(Vec::is_empty))
    {
        diagnostics.push("Some samples have no captured frames.".into());
    }
    let result = json!({"clock_domain":clock_domain,"clock_domains":clock_domains,"weight_unit":unit,"units":units,"bounds":bounds_value(bounds),"window":bounds_value(window),"sample_count":count,"total_sample_count":samples.len(),"total_weight":bounds.map(|_|total_weight),"capture_weight":bounds.map(|_|capture_weight),"unaligned_count":unaligned,"overview":overview,"activity":activity,"functions_truncated":functions.len()<page.total_functions,"total_functions":page.total_functions,"functions":functions,"roots":roots,"tree_truncated":tree_truncated,"selected":selected.finish(selected_id,&unit),"diagnostics":diagnostics});
    work.check()?;
    Ok(result)
}

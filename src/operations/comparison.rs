use crate::{
    model::runs::{Run, RunStatus},
    query,
    storage::Store,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn elapsed(run: &Run) -> Option<f64> {
    if run.runtime == "imported" {
        return None;
    }
    run.finished_at_us?
        .checked_sub(run.started_at_us)
        .map(|duration| duration as f64 / 1000.0)
}
struct ResourceValue {
    value: f64,
    observations: usize,
    lifetimes: usize,
    minimum_points_per_lifetime: usize,
    identity_known: bool,
}

#[derive(Default)]
struct SampleWeight {
    observed: bool,
    value: Option<f64>,
}

impl SampleWeight {
    fn add(&mut self, value: Option<f64>) {
        self.value = if self.observed {
            self.value
                .zip(value)
                .map(|(before, next)| before + next)
                .filter(|sum| sum.is_finite())
        } else {
            value
        };
        self.observed = true;
    }

    fn measured(&self, complete: bool) -> Option<f64> {
        if self.observed {
            self.value
        } else if complete {
            Some(0.0)
        } else {
            None
        }
    }
}

fn resource_value(
    rows: &[Value],
    name: &str,
    unit: &str,
    cumulative: bool,
    work: &mut query::WorkBudget,
) -> Result<Option<ResourceValue>> {
    let mut lifetimes: BTreeMap<String, (f64, BTreeSet<u64>)> = BTreeMap::new();
    let mut identity_known = true;
    for row in rows.iter().filter(|r| {
        r["kind"] == "metric"
            && r["name"] == name
            && r["unit"] == unit
            && r["scope"] == "process"
            && r["attributes"]["process_role"] != "collector"
    }) {
        work.check()?;
        if cumulative && row["attributes"]["temporality"] != "cumulative" {
            continue;
        }
        let Some(value) = row["value"].as_f64() else {
            continue;
        };
        let attrs = &row["attributes"];
        let lifetime = if let Some(id) = attrs["process_lifetime_id"].as_str() {
            json!([row["pid"], id]).to_string()
        } else if let Some(start) = attrs["process_started_at_seconds"]
            .as_u64()
            .filter(|t| *t > 0)
        {
            json!([row["pid"], start]).to_string()
        } else {
            identity_known = false;
            json!(["unknown lifetime", row["pid"], row["producer_id"]]).to_string()
        };
        if !lifetimes.contains_key(&lifetime) {
            work.entry()?;
        }
        let entry = lifetimes
            .entry(lifetime)
            .or_insert((value, BTreeSet::new()));
        entry.0 = entry.0.max(value);
        if let Some(timestamp) = row["timestamp_us"].as_u64()
            && !entry.1.contains(&timestamp)
        {
            work.entry()?;
            entry.1.insert(timestamp);
        }
    }
    if lifetimes.is_empty() {
        return Ok(None);
    }
    Ok(Some(ResourceValue {
        value: if cumulative {
            lifetimes.values().map(|(value, _)| value).sum()
        } else {
            lifetimes
                .values()
                .map(|(value, _)| *value)
                .reduce(f64::max)
                .unwrap()
        },
        observations: lifetimes.values().map(|(_, times)| times.len()).sum(),
        lifetimes: lifetimes.len(),
        minimum_points_per_lifetime: lifetimes
            .values()
            .map(|(_, times)| times.len())
            .min()
            .unwrap(),
        identity_known,
    }))
}

pub fn compare(
    store: &Store,
    before: &str,
    after: &str,
    requested_unit: Option<&str>,
    checkpoint: impl FnMut() -> Result<()> + Send + Clone + 'static,
) -> Result<Value> {
    let before = store.run(before)?;
    let after = store.run(after)?;
    let mut reasons = Vec::new();
    if before.command != after.command {
        reasons.push("Workload commands differ");
    }
    if before.cwd != after.cwd {
        reasons.push("Project working directories differ");
    }
    if before.runtime != after.runtime
        || before.source["runtime_version"] != after.source["runtime_version"]
    {
        reasons.push("Runtime identities differ");
    }
    if before.correctness["passed"] != true || after.correctness["passed"] != true {
        reasons.push("Successful correctness checks are required for both runs");
    }
    if before.status != RunStatus::Completed || after.status != RunStatus::Completed {
        reasons.push("Both recordings must complete successfully");
    }
    let known = |v: &Value| v.as_str().is_some_and(|s| !s.is_empty() && s != "unknown");
    if !known(&before.source["fingerprint"]) || !known(&after.source["fingerprint"]) {
        reasons.push("Source/build identity is unavailable");
    }
    if !known(&before.source["collector_settings_fingerprint"])
        || !known(&after.source["collector_settings_fingerprint"])
    {
        reasons.push("Collector settings identity is unavailable");
    } else if before.source["collector_settings_fingerprint"]
        != after.source["collector_settings_fingerprint"]
    {
        reasons.push("Collector settings differ");
    }
    for (a, b, missing, different) in [
        (
            &before.source["environment_fingerprint"],
            &after.source["environment_fingerprint"],
            "Effective environment identity is unavailable",
            "Effective workload environments differ",
        ),
        (
            &before.source["workload_inputs"]["fingerprint"],
            &after.source["workload_inputs"]["fingerprint"],
            "Declared workload input identity is unavailable",
            "Declared workload input contents differ",
        ),
        (
            &before.source["correctness_recipe_fingerprint"],
            &after.source["correctness_recipe_fingerprint"],
            "Correctness recipe identity is unavailable",
            "Correctness recipes differ",
        ),
    ] {
        if !known(a) || !known(b) {
            reasons.push(missing);
        } else if a != b {
            reasons.push(different);
        }
    }
    let supervised_before = before.source["supervised_elapsed_us"].as_f64();
    let supervised_after = after.source["supervised_elapsed_us"].as_f64();
    let supervised_scope_known = known(&before.source["supervised_execution_scope"])
        && known(&after.source["supervised_execution_scope"]);
    let supervised_scope_matches =
        before.source["supervised_execution_scope"] == after.source["supervised_execution_scope"];
    if (supervised_before.is_some() || supervised_after.is_some()) && !supervised_scope_known {
        reasons.push("Supervised elapsed measurement scope is unavailable");
    } else if !supervised_scope_matches {
        reasons.push("Supervised elapsed measurement scopes differ");
    }
    let metric = |name: &str,
                  unit: &str,
                  scope: &str,
                  basis: &str,
                  a: Option<f64>,
                  b: Option<f64>| {
        let pair = a.zip(b);
        json!({"name":name,"unit":unit,"scope":scope,"basis":basis,"before":a,"after":b,"delta":pair.map(|(a,b)|b-a),"percent_change":pair.filter(|(a,_)|*a!=0.0).map(|(a,b)|(b-a)/a*100.0),"comparable":pair.is_some()})
    };
    let mut metrics = vec![metric(
        "recording.elapsed",
        "milliseconds",
        "recording_orchestration",
        "record creation through preservation/ingestion; not target wall time",
        elapsed(&before),
        elapsed(&after),
    )];
    if metrics[0]["comparable"] == false {
        metrics[0]["measurement_quality"] = json!("unavailable");
        metrics[0]["diagnostic"] = json!(if before.runtime == "imported"
            || after.runtime == "imported"
        {
            "Imported profile timestamps describe ingestion, not a measured execution duration."
        } else {
            "Recording completion timestamps are unavailable or inconsistent; an absent duration is not a measured zero."
        });
    }
    if supervised_before.is_some() || supervised_after.is_some() {
        let mut row = metric(
            "supervised.elapsed",
            "milliseconds",
            before.source["supervised_execution_scope"]
                .as_str()
                .unwrap_or("unknown"),
            "supervised process spawn/wait including runtime or collector overhead; not isolated hot-function time",
            supervised_before.map(|value| value / 1000.0),
            supervised_after.map(|value| value / 1000.0),
        );
        if !supervised_scope_known || !supervised_scope_matches {
            row["comparable"] = json!(false);
            row["delta"] = Value::Null;
            row["percent_change"] = Value::Null;
            row["measurement_quality"] = json!("unknown_scope");
            row["diagnostic"] = json!(
                "Supervised measurement scopes are missing or differ; their elapsed values cannot establish a shared duration comparison."
            );
        } else if row["comparable"] == false {
            row["measurement_quality"] = json!("unavailable");
            row["diagnostic"] = json!(
                "Supervised elapsed measurement is unavailable for one recording; absence is not a measured zero."
            );
        }
        metrics.push(row);
    }
    let limits = query::QueryLimits::default();
    let (a, b) = query::comparison_inputs(
        store,
        &before.id,
        &after.id,
        limits.read.clone(),
        checkpoint.clone(),
    )?;
    let mut work = query::WorkBudget::new(limits, checkpoint);
    let a = a.value;
    let b = b.value;
    for (name, input, unit, cumulative, scope, basis) in [
        (
            "process.rss.max_process",
            "process.rss",
            "bytes",
            false,
            "observed_measured_process",
            "maximum observed individual-process RSS; collector wrapper and non-process scopes excluded",
        ),
        (
            "process.cpu_time.observed_total",
            "process.cpu_time",
            "milliseconds",
            true,
            "observed_measured_process_lifetimes",
            "sum of last cumulative CPU observations per OS process lifetime; collector wrapper excluded; final cumulative values may be missed",
        ),
    ] {
        work.check()?;
        let x = resource_value(&a, input, unit, cumulative, &mut work)?;
        let y = resource_value(&b, input, unit, cumulative, &mut work)?;
        work.check()?;
        if x.is_some() || y.is_some() {
            let sufficient = x.as_ref().zip(y.as_ref()).is_some_and(|(x, y)| {
                x.identity_known
                    && y.identity_known
                    && x.minimum_points_per_lifetime >= 2
                    && y.minimum_points_per_lifetime >= 2
            });
            let mut row = metric(
                name,
                unit,
                scope,
                basis,
                x.as_ref().map(|v| v.value),
                y.as_ref().map(|v| v.value),
            );
            row["measurement_quality"] = json!(if sufficient {
                "observed_partial"
            } else if x.is_none() || y.is_none() {
                "unavailable"
            } else {
                "insufficient_observations"
            });
            row["comparable"] = json!(sufficient);
            row["before_observations"] = json!(x.as_ref().map_or(0, |v| v.observations));
            row["after_observations"] = json!(y.as_ref().map_or(0, |v| v.observations));
            row["before_process_lifetimes"] = json!(x.as_ref().map_or(0, |v| v.lifetimes));
            row["after_process_lifetimes"] = json!(y.as_ref().map_or(0, |v| v.lifetimes));
            if !sufficient {
                row["delta"] = Value::Null;
                row["percent_change"] = Value::Null;
                row["diagnostic"] = json!(if x.is_none() || y.is_none() {
                    "Process measurement is unavailable for one recording; absence is not a measured zero."
                } else {
                    "Fewer than two distinct observations per process lifetime, or unknown lifetime identity; startup-only points do not establish CPU totals or memory peaks."
                });
            }
            metrics.push(row);
        }
    }
    let units = |rows: &[Value]| {
        rows.iter()
            .filter(|r| r["kind"] == "sample")
            .filter_map(|r| r["weight_unit"].as_str())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    };
    let before_units = units(&a);
    let after_units = units(&b);
    if requested_unit.is_none() && before_units != after_units {
        reasons.push("Available sampled weight units differ or one recording has no samples");
    }
    const HOTSPOT_LIMIT: usize = 1000;
    let before_page = query::comparison_hotspots_checked(
        &a,
        requested_unit.unwrap_or(""),
        HOTSPOT_LIMIT,
        &mut work,
    )?;
    let unit = before_page.result["weight_unit"].as_str().unwrap();
    let after_page = query::comparison_hotspots_checked(&b, unit, HOTSPOT_LIMIT, &mut work)?;
    let before_available = before_units.contains(unit);
    let after_available = after_units.contains(unit);
    let before_truncated = before_page.total_functions > HOTSPOT_LIMIT;
    let after_truncated = after_page.total_functions > HOTSPOT_LIMIT;
    let before_complete = before_available && !before_truncated;
    let after_complete = after_available && !after_truncated;
    if requested_unit.is_some() && (!before_available || !after_available) {
        reasons.push("Requested sampled weight unit is unavailable for one or both recordings");
    }
    let mut joined: BTreeMap<String, (Value, SampleWeight, SampleWeight)> = BTreeMap::new();
    for (profile, is_after) in [(&before_page.result, false), (&after_page.result, true)] {
        for row in profile["hotspots"].as_array().unwrap() {
            let key = json!([row["function"], row["file"], row["module"]]).to_string();
            let entry = joined.entry(key).or_insert_with(|| {
                (
                    row.clone(),
                    SampleWeight::default(),
                    SampleWeight::default(),
                )
            });
            if is_after {
                entry.2.add(row["self_weight"].as_f64());
            } else {
                entry.1.add(row["self_weight"].as_f64());
            }
        }
    }
    let hotspots=joined.into_values().map(|(r,x,y)| {
        let presence = if x.observed && y.observed {"both_observed"} else if x.observed && after_complete {"before_only_observed"} else if y.observed && before_complete {"after_only_observed"} else {"unknown"};
        let before = x.measured(before_complete);
        let after = y.measured(after_complete);
        json!({"function":r["function"],"file":r["file"],"module":r["module"],"before":before,"after":after,"delta":before.zip(after).map(|(before,after)|after-before),"unit":unit,"basis":"profiler sample weight grouped by function/file/module across measured code versions; not elapsed time or a speedup ratio","before_available":before.is_some(),"after_available":after.is_some(),"presence":presence})
    }).collect::<Vec<_>>();
    let hotspot_selection = json!({"weight_unit":unit,"limit_per_recording":HOTSPOT_LIMIT,"before_available":before_available,"after_available":after_available,"before_total_functions":before_page.total_functions,"after_total_functions":after_page.total_functions,"before_truncated":before_truncated,"after_truncated":after_truncated});
    work.check()?;
    Ok(
        json!({"before":before,"after":after,"comparable":reasons.is_empty(),"reasons":reasons,"metrics":metrics,"correctness":{"before":before.correctness,"after":after.correctness},"hotspots":hotspots,"hotspot_selection":hotspot_selection,"sample_weight_units":{"before":before_units,"after":after_units},"limitations":["Single profiled runs are descriptive comparisons; repeated low-intervention measurements are required to establish a speed improvement.","Source/build changes are permitted as the optimization under comparison; commands, environment, declared inputs, collector settings and correctness recipe must match.","Only explicitly declared input files are fingerprinted; dynamic and network inputs are not verified.","Exit-code-only recipes establish successful termination, not semantic output correctness; use an explicit expected stdout value or digest when output is part of the contract.","Process resource peaks retain process scope; no per-request ownership is inferred.","Hotspots group observed function/file/module names across measured code versions; this grouping does not establish semantic equivalence. A missing row outside a truncated rank window is unknown, not an unobserved function."]}),
    )
}

#[cfg(test)]
mod work_tests {
    use super::*;
    #[test]
    fn resource_comparison_counts_distinct_lifetimes_and_observation_points() {
        let rows=(1..=2).map(|timestamp|json!({"kind":"metric","name":"process.rss","unit":"bytes","scope":"process","pid":1,"timestamp_us":timestamp,"value":timestamp*5,"attributes":{"process_lifetime_id":"original","process_role":"measured"}})).collect::<Vec<_>>();
        let mut budget = query::WorkBudget::new(
            query::QueryLimits {
                maximum_aggregate_entries: 2,
                ..Default::default()
            },
            || Ok(()),
        );
        let error = resource_value(&rows, "process.rss", "bytes", false, &mut budget)
            .err()
            .unwrap();
        let limit = error.downcast_ref::<query::WorkBudgetExceeded>().unwrap();
        assert_eq!(limit.dimension, query::WorkDimension::AggregateEntries);
        assert_eq!(limit.observed, 3);
        let mut budget = query::WorkBudget::new(
            query::QueryLimits {
                maximum_aggregate_entries: 3,
                ..Default::default()
            },
            || Ok(()),
        );
        let value = resource_value(&rows, "process.rss", "bytes", false, &mut budget)
            .unwrap()
            .unwrap();
        assert_eq!(value.value, 10.0);
        assert_eq!(value.observations, 2);
        assert_eq!(value.lifetimes, 1);
    }
}

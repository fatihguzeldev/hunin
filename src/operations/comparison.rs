use crate::{model::runs::Run, query, storage::Store};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn elapsed(run: &Run) -> f64 {
    run.finished_at_us
        .unwrap_or(run.started_at_us)
        .saturating_sub(run.started_at_us) as f64
        / 1000.0
}
struct ResourceValue {
    value: f64,
    observations: usize,
    lifetimes: usize,
    minimum_points_per_lifetime: usize,
    identity_known: bool,
}

fn resource_value(
    rows: &[Value],
    name: &str,
    unit: &str,
    cumulative: bool,
) -> Option<ResourceValue> {
    let mut lifetimes: BTreeMap<String, (f64, BTreeSet<u64>)> = BTreeMap::new();
    let mut identity_known = true;
    for row in rows.iter().filter(|r| {
        r["kind"] == "metric"
            && r["name"] == name
            && r["unit"] == unit
            && r["scope"] == "process"
            && r["attributes"]["process_role"] != "collector"
    }) {
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
        let entry = lifetimes
            .entry(lifetime)
            .or_insert((value, BTreeSet::new()));
        entry.0 = entry.0.max(value);
        if let Some(timestamp) = row["timestamp_us"].as_u64() {
            entry.1.insert(timestamp);
        }
    }
    if lifetimes.is_empty() {
        return None;
    }
    Some(ResourceValue {
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
    })
}

pub fn compare(store: &Store, before: &str, after: &str) -> Result<Value> {
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
    if before.status != "completed" || after.status != "completed" {
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
    if before.source["supervised_execution_scope"] != after.source["supervised_execution_scope"] {
        reasons.push("Supervised elapsed measurement scopes differ");
    }
    let metric = |name: &str, unit: &str, scope: &str, basis: &str, a: f64, b: f64| json!({"name":name,"unit":unit,"scope":scope,"basis":basis,"before":a,"after":b,"delta":b-a,"percent_change":if a!=0.0 {Some((b-a)/a*100.0)}else{None}});
    let mut metrics = vec![metric(
        "recording.elapsed",
        "milliseconds",
        "recording_orchestration",
        "record creation through preservation/ingestion; not target wall time",
        elapsed(&before),
        elapsed(&after),
    )];
    if let (Some(x), Some(y)) = (
        before.source["supervised_elapsed_us"].as_f64(),
        after.source["supervised_elapsed_us"].as_f64(),
    ) {
        metrics.push(metric(
            "supervised.elapsed",
            "milliseconds",
            before.source["supervised_execution_scope"].as_str().unwrap_or("unknown"),
            "supervised process spawn/wait including runtime or collector overhead; not isolated hot-function time",
            x / 1000.0,
            y / 1000.0,
        ));
    }
    let a = store.records(&before.id)?;
    let b = store.records(&after.id)?;
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
        if let (Some(x), Some(y)) = (
            resource_value(&a, input, unit, cumulative),
            resource_value(&b, input, unit, cumulative),
        ) {
            let sufficient = x.identity_known
                && y.identity_known
                && x.minimum_points_per_lifetime >= 2
                && y.minimum_points_per_lifetime >= 2;
            let mut row = metric(name, unit, scope, basis, x.value, y.value);
            row["measurement_quality"] = json!(if sufficient {
                "observed_partial"
            } else {
                "insufficient_observations"
            });
            row["comparable"] = json!(sufficient);
            row["before_observations"] = json!(x.observations);
            row["after_observations"] = json!(y.observations);
            row["before_process_lifetimes"] = json!(x.lifetimes);
            row["after_process_lifetimes"] = json!(y.lifetimes);
            if !sufficient {
                row["delta"] = Value::Null;
                row["percent_change"] = Value::Null;
                row["diagnostic"] = json!(
                    "Fewer than two distinct observations per process lifetime, or unknown lifetime identity; startup-only points do not establish CPU totals or memory peaks."
                );
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
    if units(&a) != units(&b) {
        reasons.push("Available sampled weight units differ or one recording has no samples");
    }
    let ah = query::hotspots(&a, "", 1000);
    let bh = query::hotspots(&b, ah["weight_unit"].as_str().unwrap_or("samples"), 1000);
    let mut joined: BTreeMap<String, (Value, f64, f64)> = BTreeMap::new();
    for (profile, is_after) in [(&ah, false), (&bh, true)] {
        for row in profile["hotspots"].as_array().unwrap() {
            let key = json!([row["function"], row["file"], row["module"]]).to_string();
            let entry = joined.entry(key).or_insert((row.clone(), 0.0, 0.0));
            if is_after {
                entry.2 += row["self_weight"].as_f64().unwrap_or(0.0);
            } else {
                entry.1 += row["self_weight"].as_f64().unwrap_or(0.0);
            }
        }
    }
    let hotspots=joined.into_values().map(|(r,x,y)|json!({"function":r["function"],"file":r["file"],"module":r["module"],"before":x,"after":y,"delta":y-x,"unit":bh["weight_unit"],"basis":"profiler sample weight; not elapsed time or a speedup ratio","before_available":!units(&a).is_empty(),"after_available":!units(&b).is_empty()})).collect::<Vec<_>>();
    Ok(
        json!({"before":before,"after":after,"comparable":reasons.is_empty(),"reasons":reasons,"metrics":metrics,"correctness":{"before":before.correctness,"after":after.correctness},"hotspots":hotspots,"sample_weight_units":{"before":units(&a),"after":units(&b)},"limitations":["Single profiled runs are descriptive comparisons; repeated low-intervention measurements are required to establish a speed improvement.","Source/build changes are permitted as the optimization under comparison; commands, environment, declared inputs, collector settings and correctness recipe must match.","Only explicitly declared input files are fingerprinted; dynamic and network inputs are not verified.","Exit-code-only recipes establish successful termination, not semantic output correctness; use an explicit expected stdout value or digest when output is part of the contract.","Process resource peaks retain process scope; no per-request ownership is inferred."]}),
    )
}

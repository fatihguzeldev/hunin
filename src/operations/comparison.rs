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
    let metric = |name: &str, unit: &str, a: f64, b: f64| json!({"name":name,"unit":unit,"before":a,"after":b,"delta":b-a,"percent_change":if a!=0.0 {Some((b-a)/a*100.0)}else{None}});
    let mut metrics = vec![metric(
        "recording.elapsed",
        "milliseconds",
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
            x / 1000.0,
            y / 1000.0,
        ));
    }
    let a = store.records(&before.id)?;
    let b = store.records(&after.id)?;
    let measured =
        |r: &&Value| r["kind"] == "metric" && r["attributes"]["process_role"] != "collector";
    let rss = |rows: &[Value]| {
        rows.iter()
            .filter(measured)
            .filter(|r| r["name"] == "process.rss")
            .filter_map(|r| r["value"].as_f64())
            .reduce(f64::max)
    };
    if let (Some(x), Some(y)) = (rss(&a), rss(&b)) {
        metrics.push(metric("process.rss.max_process", "bytes", x, y));
    }
    let cpu = |rows: &[Value]| {
        let mut lifetimes: BTreeMap<String, f64> = BTreeMap::new();
        for r in rows
            .iter()
            .filter(measured)
            .filter(|r| r["name"] == "process.cpu_time")
        {
            if let (Some(id), Some(value)) = (r["producer_id"].as_str(), r["value"].as_f64()) {
                let current = lifetimes.entry(id.into()).or_default();
                *current = current.max(value);
            }
        }
        (!lifetimes.is_empty()).then(|| lifetimes.values().sum::<f64>())
    };
    if let (Some(x), Some(y)) = (cpu(&a), cpu(&b)) {
        metrics.push(metric(
            "process.cpu_time.observed_total",
            "milliseconds",
            x,
            y,
        ));
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
    let hotspots=joined.into_values().map(|(r,x,y)|json!({"function":r["function"],"file":r["file"],"before":x,"after":y,"delta":y-x,"unit":bh["weight_unit"],"before_available":!units(&a).is_empty(),"after_available":!units(&b).is_empty()})).collect::<Vec<_>>();
    Ok(
        json!({"before":before,"after":after,"comparable":reasons.is_empty(),"reasons":reasons,"metrics":metrics,"correctness":{"before":before.correctness,"after":after.correctness},"hotspots":hotspots,"limitations":["Single profiled runs are descriptive comparisons; repeated low-intervention measurements are required to establish a speed improvement.","Process resource peaks retain process scope; no per-request ownership is inferred."]}),
    )
}

use super::{checked_us, sample};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub(super) fn decode(
    path: &Path,
    raw: &Value,
    run_id: &str,
    key: &str,
) -> Result<(Vec<Value>, Vec<String>)> {
    if raw["tool"] != "cpusampler" {
        bail!("expected Graal CPUSampler JSON");
    }
    let mut records = Vec::new();
    let mut aggregate = false;
    let metadata: Value = fs::read(path.with_extension("meta.json"))
        .ok()
        .map(|data| serde_json::from_slice(&data))
        .transpose()?
        .unwrap_or(json!({}));
    let pid = metadata["pid"].as_u64().unwrap_or(0);
    for context in raw["contexts"]
        .as_array()
        .context("Graal CPU sampler contexts missing")?
    {
        let interval = context["period"].as_f64().unwrap_or(10.0) * 1000.0;
        for thread in context["profile"]
            .as_array()
            .context("Graal thread profiles missing")?
        {
            let tid = thread["thread"].as_str().unwrap_or("unknown");
            for root in thread["samples"]
                .as_array()
                .context("Graal samples missing")?
            {
                walk(
                    root,
                    &[],
                    tid,
                    pid,
                    interval,
                    run_id,
                    key,
                    &mut records,
                    &mut aggregate,
                    0,
                )?;
            }
        }
    }
    let mut diagnostics =
        vec!["Graal thread labels identify guest threads rather than OS thread IDs".into()];
    if pid == 0 {
        diagnostics.push("Imported raw CPUSampler output has no process identity sidecar; OS PID remains unknown".into());
    }
    if aggregate {
        diagnostics.push(
            "Graal profile has aggregate weights without hit timestamps; sample time is unresolved"
                .into(),
        );
    }
    Ok((records, diagnostics))
}

fn walk(
    node: &Value,
    parent: &[Value],
    tid: &str,
    pid: u64,
    interval: f64,
    run_id: &str,
    key: &str,
    records: &mut Vec<Value>,
    aggregate: &mut bool,
    depth: usize,
) -> Result<()> {
    if depth > 4096 {
        bail!("Graal stack exceeds supported depth");
    }
    let source = &node["source_section"];
    let mut frame = json!({"function":node["root_name"].as_str().unwrap_or("[unknown]")});
    if let Some(file) = source["path"].as_str() {
        frame["file"] = json!(file);
    }
    if let Some(line) = source["start_line"].as_u64().filter(|n| *n > 0) {
        frame["line"] = json!(line);
    }
    if let Some(column) = source["start_column"].as_u64().filter(|n| *n > 0) {
        frame["column"] = json!(column - 1);
    }
    let mut stack = parent.to_vec();
    stack.push(frame);
    let count = node["self_hit_count"].as_u64().unwrap_or(0);
    let hits = node["self_hit_times"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if !hits.is_empty() && hits.len() as u64 != count {
        bail!("Graal self hit count and timestamps disagree");
    }
    if count > 0 {
        if hits.is_empty() {
            *aggregate = true;
            let mut record = sample(
                run_id,
                key,
                records.len(),
                pid,
                tid,
                0,
                stack.clone(),
                count as f64,
                "samples",
                json!({"source_format":"graal-cpusampler","clock":"unknown","clock_alignment":"unmapped","temporal_resolution":"aggregate","tid_kind":"graal_thread_label"}),
            );
            record["interval_us"] = json!(interval);
            records.push(record);
        } else {
            for hit in hits {
                let timestamp = checked_us(
                    hit.as_f64().context("invalid Graal hit timestamp")? * 1000.0,
                    "Graal epoch hit time",
                )?;
                let mut record = sample(
                    run_id,
                    key,
                    records.len(),
                    pid,
                    tid,
                    timestamp,
                    stack.clone(),
                    1.0,
                    "samples",
                    json!({"source_format":"graal-cpusampler","clock":"unix_epoch","clock_alignment":"collector_epoch_ms","tid_kind":"graal_thread_label"}),
                );
                record["interval_us"] = json!(interval);
                records.push(record);
            }
        }
    }
    for child in node["children"].as_array().into_iter().flatten() {
        walk(
            child,
            &stack,
            tid,
            pid,
            interval,
            run_id,
            key,
            records,
            aggregate,
            depth + 1,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn self_hits_avoid_double_counting_inclusive_ancestors() {
        let raw = json!({"tool":"cpusampler","contexts":[{"period":10,"profile":[{"thread":"main","samples":[{"root_name":"parent","hit_count":2,"self_hit_count":0,"children":[{"root_name":"hot","self_hit_count":2,"self_hit_times":[1700000000000u64,1700000000010u64],"source_section":{"path":"work.py","start_line":4}}]}]}]}]});
        let (r, _) = decode(Path::new("missing.json"), &raw, "run", "key").unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["timestamp_us"], 1700000000000000u64);
        assert_eq!(r[0]["stack"][1]["function"], "hot");
    }
}

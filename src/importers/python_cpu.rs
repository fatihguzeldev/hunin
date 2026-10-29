use super::output::RecordBatch;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::Path,
};

pub(super) fn decode_into(
    path: &Path,
    run_id: &str,
    key: &str,
    records: &mut RecordBatch<'_>,
) -> Result<Vec<String>> {
    let metadata_path = std::path::PathBuf::from(format!("{}.meta.json", path.display()));
    let metadata: Value = serde_json::from_slice(
        &fs::read(&metadata_path).context("CPU progress metadata missing")?,
    )?;
    if metadata["format"] != "python-cpu-progress-v1" {
        bail!("unsupported Python CPU progress metadata format");
    }
    if metadata["errors"]
        .as_array()
        .is_some_and(|errors| !errors.is_empty())
    {
        bail!("Python CPU sampler failed: {}", metadata["errors"]);
    }
    let pid = metadata["pid"]
        .as_u64()
        .context("CPU sampler PID missing")?;
    let origin = metadata["epoch_origin_ns"]
        .as_i64()
        .context("CPU sampler epoch calibration missing")? as i128;
    let uncertainty = metadata["clock_uncertainty_ns"]
        .as_u64()
        .context("CPU sampler calibration uncertainty missing")?;
    let max_gap = metadata["max_gap_ns"]
        .as_u64()
        .context("CPU sampler maximum interval missing")?;
    let max_latency = metadata["max_capture_latency_ns"]
        .as_u64()
        .context("CPU sampler latency bound missing")?;
    for name in ["clock_source", "coverage", "weight_semantics"] {
        records.fragment(&metadata[name])?;
    }
    let mut sample_count = 0;
    let mut ended_thread_gaps = 0;
    for (index, line) in BufReader::new(fs::File::open(path)?).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let row: Value = serde_json::from_str(&line)
            .with_context(|| format!("CPU progress record {}", index + 1))?;
        if row["record_type"] == "unmapped_thread_end_counter" {
            ended_thread_gaps += 1;
            continue;
        }
        if row["record_type"] != "observation" {
            bail!("unexpected CPU progress record kind");
        }
        let native = row["native_tid"]
            .as_u64()
            .context("native thread ID missing")?;
        let tid = row["thread_ident"]
            .as_str()
            .context("Python thread identity missing")?;
        let lifetime = row["thread_lifetime"]
            .as_str()
            .context("OS thread lifetime identity missing")?;
        let producer = format!("python-cpu:{pid}:{native}:{lifetime}");
        let timestamp = row["timestamp_ns"]
            .as_u64()
            .context("CPU observation timestamp missing")?;
        let epoch = (origin + timestamp as i128) / 1000;
        if epoch < 0 || epoch > u64::MAX as i128 {
            bail!("CPU observation epoch calibration overflows timestamp");
        }
        let cpu = row["cpu_counter_ns"]
            .as_u64()
            .context("CPU counter missing")?;
        let delta = row["cpu_delta_ns"].as_u64().context("CPU delta missing")?;
        let interval = row["interval_ns"]
            .as_u64()
            .context("CPU observation interval missing")?;
        let latency = row["capture_latency_ns"]
            .as_u64()
            .context("CPU capture latency missing")?;
        records.fragment(&row["attribution"])?;
        records.fragment(&tid)?;
        records.fragment(&lifetime)?;
        let attrs = json!({"source_format":"python-cpu-progress","clock":"unix_epoch","original_clock":"runtime_monotonic","clock_source":metadata["clock_source"],"clock_alignment":"calibrated_epoch","clock_uncertainty_us":uncertainty.div_ceil(1000),"timestamp_uncertainty_us":uncertainty.saturating_add(latency).div_ceil(1000),"raw_timestamp_ns":timestamp,"frame_capture_started_ns":row["frame_capture_started_ns"],"frame_capture_finished_ns":row["frame_capture_finished_ns"],"counter_read_finished_ns":row["read_finished_ns"],"native_tid":native,"thread_id_source":"python_thread_ident","thread_lifetime_id":lifetime,"process_role":"measured","cpu_counter_ns":cpu,"cpu_delta_ns":delta,"observation_interval_ns":interval,"capture_latency_ns":latency,"attribution":row["attribution"],"sampling_basis":"os_thread_cpu_progress_gated_stack_sample","cost_semantics":metadata["weight_semantics"],"coverage":metadata["coverage"]});
        let mut metric_attrs = attrs.clone();
        metric_attrs["temporality"] = json!("cumulative");
        records.push(json!({"schema_version":1,"kind":"metric","id":format!("{key}:cpu-counter:{index}"),"run_id":run_id,"producer_id":producer,"pid":pid,"tid":tid,"timestamp_us":epoch as u64,"basis":"observed","name":"thread.cpu_time","scope":"thread","value":cpu as f64/1000.0,"unit":"microseconds","attributes":metric_attrs}))?;
        if row["attribution"] == "sample" {
            if delta == 0 || interval == 0 || interval > max_gap || latency > max_latency {
                bail!("CPU sample exceeds its declared attribution bounds");
            }
            let stack = row["stack"]
                .as_array()
                .filter(|s| !s.is_empty())
                .context("attributed CPU sample has no captured stack")?;
            let stack = records.clone_stack(stack)?;
            records.push(json!({"schema_version":1,"kind":"sample","id":format!("{key}:cpu-sample:{index}"),"run_id":run_id,"producer_id":producer,"pid":pid,"tid":tid,"timestamp_us":epoch as u64,"basis":"sampled","stack":stack,"weight":delta as f64/1000.0,"weight_unit":"microseconds","attributes":attrs}))?;
            sample_count += 1;
        }
    }
    let mut diagnostics=vec!["Python CPU weights are measured OS thread counter deltas statistically attributed across bounded observation windows; they are not individual call durations. Native-only and undiscovered short-lived threads can be absent.".into()];
    let observed = metadata["observed_cpu_ns"].as_u64().unwrap_or(0);
    let attributed = metadata["attributed_cpu_ns"].as_u64().unwrap_or(0);
    diagnostics.push(format!("Python CPU attribution coverage: {attributed} of {observed} observed thread-CPU nanoseconds; {} delayed observation windows retained without stack attribution",metadata["gap_observations"].as_u64().unwrap_or(0)));
    if ended_thread_gaps > 0 {
        diagnostics.push(format!("{ended_thread_gaps} terminated native-thread counters retained only in raw evidence: their Python identity and final CPU delta remain unmapped, and no frame costs were assigned."));
    }
    if sample_count == 0 {
        diagnostics.push("No positive CPU progress could be attributed within the declared bounds; CPU stack costs were not invented.".into());
    }
    Ok(diagnostics)
}

#[cfg(test)]
fn decode(path: &Path, run_id: &str, key: &str) -> Result<(Vec<Value>, Vec<String>)> {
    let mut budget = super::OutputBudget::default();
    let mut records = RecordBatch::new(&mut budget, None);
    let diagnostics = decode_into(path, run_id, key, &mut records)?;
    Ok((records.finish()?, diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_cpu_counter_evidence_but_rejects_stack_attribution_beyond_the_latency_budget() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("cpu.pythoncpu.jsonl");
        let metadata = json!({"format":"python-cpu-progress-v1","pid":42,"epoch_origin_ns":1000000000,"clock_uncertainty_ns":2000,"max_gap_ns":100000000,"max_capture_latency_ns":10000000,"errors":[],"observed_cpu_ns":5000,"attributed_cpu_ns":5000});
        fs::write(
            format!("{}.meta.json", path.display()),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        let mut row = json!({"record_type":"observation","native_tid":8,"thread_ident":"python-8","thread_lifetime":"created-8","timestamp_ns":100000,"cpu_counter_ns":15000,"cpu_delta_ns":5000,"interval_ns":10000,"capture_latency_ns":500,"attribution":"sample","stack":[{"function":"hot","file":"app.py","line":3,"code_id":"code"}]});
        fs::write(&path, row.to_string()).unwrap();
        let (records, _) = decode(&path, "run", "input").unwrap();
        assert_eq!(records[0]["name"], "thread.cpu_time");
        assert_eq!(records[1]["weight"], 5.0);
        assert_eq!(records[1]["timestamp_us"], 1000100);
        row["capture_latency_ns"] = json!(10000001);
        fs::write(&path, row.to_string()).unwrap();
        assert!(decode(&path, "run", "input").is_err());
        row["attribution"] = json!("unattributed_gap");
        fs::write(&path, row.to_string()).unwrap();
        let (records, _) = decode(&path, "run", "input").unwrap();
        assert_eq!(
            records.len(),
            1,
            "delayed CPU remains a counter observation, never a later-frame cost"
        );
        let ended = json!({"record_type":"unmapped_thread_end_counter","native_tid":8,"thread_lifetime":"created-8","cpu_counter_ns":90000,"ended_filetime_100ns":99});
        fs::write(&path, format!("{row}\n{ended}\n")).unwrap();
        let (records, diagnostics) = decode(&path, "run", "input").unwrap();
        assert_eq!(
            records.len(),
            1,
            "an ended handle cannot acquire a new Python identity"
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.contains("terminated native-thread counters"))
        );
    }
}

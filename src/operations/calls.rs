//! Requested counting is a separate owned recording, never a query side effect.
use super::errors::Failure;
use crate::{
    collection::CaptureRequest,
    execution::{Engine, StartRequest},
    model::{
        calls::{CountPlan, CountRequest},
        numbers::unsigned_integer,
        runs::Run,
    },
    storage::{ReadLimits, Store},
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(super) fn options(params: &Value) -> Result<CountRequest> {
    let mut value = params.clone();
    let fields = value
        .as_object_mut()
        .context("count parameters must be an object")?;
    for key in ["run_id", "timeout_seconds", "request_id"] {
        fields.remove(key);
    }
    let options: CountRequest =
        serde_json::from_value(value).map_err(Failure::invalid_parameters)?;
    options.validate().map_err(Failure::invalid_parameters)?;
    if let Some(id) = params["request_id"].as_str() {
        anyhow::ensure!(
            id.len() <= 128 && !id.contains('\0'),
            Failure::invalid_parameters("request_id must be bounded and contain no NUL")
        );
    }
    Ok(options)
}

fn parent(store: &Store, id: &str) -> Result<Run> {
    let mut run = store.run(id)?;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..16 {
        anyhow::ensure!(
            seen.insert(run.id.clone()),
            "cyclic counting parent metadata"
        );
        let Some(parent) = run.source["call_counting"]["parent_run_id"].as_str() else {
            return Ok(run);
        };
        run = store
            .run(parent)
            .context("original counting workload is no longer available")?;
    }
    bail!("counting parent chain exceeds the inspection limit")
}

fn environment(run: &Run) -> Result<BTreeMap<String, String>> {
    let overrides = &run.source["environment"]["overrides"];
    if overrides.is_null() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_value(overrides.clone())
        .context("retained explicit environment overrides are malformed")
}

pub(super) fn plan(store: &Store, params: &Value) -> Result<(Run, CountRequest, CountPlan)> {
    let options = options(params)?;
    let run = parent(store, params["run_id"].as_str().context("run_id required")?)?;
    let capture = CaptureRequest {
        command: run.command.clone(),
        cwd: run.cwd.clone().into(),
        artifact_dir: store.root().join("captures").join(&run.id),
        run_id: run.id.clone(),
        runtime: run.runtime.clone(),
        env: environment(&run)?,
    };
    let mut plan = crate::collection::counts::plan(&capture, &options)?;
    if run.status == "running" {
        plan.status = "blocked".into();
        plan.reason = Some(
            "Wait for the source recording to finish before requesting another execution".into(),
        );
    }
    plan.limitations.push("Counting uses an additional execution with newly observed source, build and inherited environment; its window is separate from the parent sampling recording.".into());
    Ok((run, options, plan))
}

pub(super) async fn measure(
    engine: Engine,
    params: Value,
    prepared: (Run, CountRequest, CountPlan),
) -> Result<Value> {
    let (parent, options, plan) = prepared;
    if plan.status != "ready" {
        return Ok(json!({"plan":plan,"parent_run_id":parent.id,"reused":false}));
    }
    let store = engine.store().clone();
    let timeout = params["timeout_seconds"].as_f64().unwrap_or(60.0);
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(
            &json!({"parent":parent.id,"options":options,"timeout_seconds":timeout})
        )?)
    );
    let key = format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{}:{}",
                parent.id,
                params["request_id"].as_str().unwrap_or(&fingerprint)
            )
            .as_bytes()
        )
    );
    let recipe = &parent.source["correctness_recipe"];
    let request: StartRequest = serde_json::from_value(json!({
        "command":parent.command,"cwd":parent.cwd,"runtime":parent.runtime,
        "name":format!("{} — function counts",parent.name),"timeout_seconds":timeout,
        "env":environment(&parent)?,
        "expected_exit_code":recipe["expected_exit_code"].as_i64().unwrap_or(0),
        "expected_stdout_sha256":recipe["expected_stdout_sha256"],
        "input_files":parent.source["workload_inputs"]["files"].as_array().into_iter().flatten().filter_map(|file|file["path"].as_str()).collect::<Vec<_>>(),
        "call_counting":options,
    }))?;
    let claim_store = store.clone();
    let claim_key = key.clone();
    let claim_hash = fingerprint.clone();
    let parent_id = parent.id.clone();
    let (claimed, mut reservation) = tokio::task::spawn_blocking(move || {
        claim_store.claim_count_request(&claim_key, &claim_hash, &parent_id)
    })
    .await??;
    if !claimed {
        if reservation["fingerprint"] != fingerprint {
            return Err(Failure::conflict(
                "request_id was already used for a different counting request",
            )
            .into());
        }
        let existing_store = store.clone();
        let existing_key = key.clone();
        let existing = tokio::task::spawn_blocking(move || {
            existing_store.counting_run_for_request(&existing_key)
        })
        .await??;
        if let Some(id) = existing {
            let read_store = store.clone();
            let run = tokio::task::spawn_blocking(move || read_store.run(&id)).await??;
            return Ok(json!({"run":run,"plan":plan,"parent_run_id":parent.id,"reused":true}));
        }
        return Err(Failure::conflict("The counting request is preparing, was interrupted, or its recording was removed. Inspect existing recordings before using a new request_id.").into());
    }
    let result = engine
        .start_counted(request, parent.id.clone(), key.clone())
        .await;
    match &result {
        Ok(run) => {
            reservation["status"] = json!("accepted");
            reservation["counting_run_id"] = json!(run.id);
        }
        Err(error) => {
            reservation["status"] = json!("failed");
            reservation["diagnostic"] = json!(format!("{error:#}"));
        }
    }
    let write_store = store.clone();
    tokio::task::spawn_blocking(move || {
        write_store.save_document("call-count-requests", &key, &reservation)
    })
    .await??;
    Ok(json!({"run":result?,"plan":plan,"parent_run_id":parent.id,"reused":false}))
}

pub(super) fn query(
    store: &Store,
    params: &Value,
    checkpoint: impl FnMut() -> Result<()> + Send + 'static,
) -> Result<Value> {
    let requested_run_id = params["run_id"].as_str().context("run_id required")?;
    let mut run = store.run(requested_run_id)?;
    if !run.source["call_counting"].is_object()
        && let Some(child) = store.latest_counting_run(requested_run_id)?
    {
        run = store.run(&child)?;
        anyhow::ensure!(
            run.source["call_counting"]["parent_run_id"] == requested_run_id,
            "counting request parent identity mismatch"
        );
    }
    let run_id = run.id.as_str();
    let counting = &run.source["call_counting"];
    let requested = counting["requested_scope"].as_str().unwrap_or("all");
    let mut method = run.source["collector_identity"]["name"].clone();
    let expected_method = method.clone();
    let mut collection_scope = "all".to_owned();
    let mut complete_event = false;
    let mut rows: BTreeMap<String, Value> = BTreeMap::new();
    let mut limitations = vec!["Counts describe the separate counting recording. Missing rows do not establish zero invocations; native extensions, inline code and unobserved producers may be outside the method's scope.".to_owned()];
    let mut diagnostics = run.diagnostics.clone();
    if counting.is_object() {
        store.with_query_snapshot(run_id, ReadLimits::default(), checkpoint, |port| {
            port.visit(&["metric", "event"], |text| {
                let record: Value = serde_json::from_str(text)?;
                if record["producer_id"] != format!("calls:{run_id}") { return Ok(()); }
                let name = record["name"].as_str().unwrap_or("");
                if !matches!((record["kind"].as_str(),name), (Some("event"),"function.counts.complete") | (Some("metric"),"function.calls" | "function.resumes")) { return Ok(()); }
                let attributes = &record["attributes"];
                anyhow::ensure!(expected_method.is_string() && attributes["method"] == expected_method, "accepted count method does not match its measured collector identity");
                anyhow::ensure!(attributes["collection_scope"] == "all_then_filter", "accepted count collection scope is unsupported");
                if let Some(value) = attributes["method"].as_str() { method = json!(value); }
                if let Some(value) = attributes["collection_scope"].as_str() { collection_scope = value.to_owned(); }
                if record["kind"] == "event" && record["name"] == "function.counts.complete" {
                    anyhow::ensure!(attributes["status"] == "complete" && !complete_event, "accepted count completion evidence is invalid or duplicated");
                    complete_event = true;
                    if let Some(items) = attributes["limitations"].as_array() {
                        limitations.extend(items.iter().filter_map(Value::as_str).map(str::to_owned));
                    }
                    return Ok(());
                }
                if record["kind"] != "metric" || !matches!(name, "function.calls" | "function.resumes") { return Ok(()); }
                let function = &attributes["function"];
                anyhow::ensure!(record["basis"] == "observed" && record["unit"] == "count" && record["scope"] == "process" && attributes["counter_temporality"] == "delta" && attributes["count_semantics"] == if name == "function.calls" { "starts" } else { "resumes" }, "accepted function counter has incompatible measurement semantics");
                let Some(function_name) = function["name"].as_str() else { bail!("accepted call counter is missing its function identity"); };
                let value = unsigned_integer(&record["value"]).context("accepted function count is not an exact integer")?;
                anyhow::ensure!(value <= 9_007_199_254_740_991, "function count exceeds the cross-language exact integer range");
                let key = serde_json::to_string(function)?;
                let entry = rows.entry(key).or_insert_with(|| json!({"function":{"name":function_name,"file":function["file"],"line":function["line"]},"calls":null,"resumes":null}));
                if let Some(code_id) = function.get("code_id") {
                    anyhow::ensure!(code_id.as_str().is_some_and(|id| !id.is_empty() && id.len() <= 4096), "accepted function code identity is invalid");
                    entry["function"]["code_id"] = code_id.clone();
                }
                let field = if name == "function.calls" {"calls"} else {"resumes"};
                anyhow::ensure!(entry[field].is_null(), "duplicate function count in one selected counting window");
                entry[field] = json!(value);
                Ok(())
            })
        })?;
    }
    let status = if !counting.is_object() {
        "not_collected"
    } else if run.status == "running" {
        "running"
    } else if run.status == "canceled" {
        "canceled"
    } else if run.correctness["passed"] == false {
        "failed"
    } else if complete_event
        && run.source["collection_status"] == "completed"
        && run.correctness["passed"] == true
    {
        "complete"
    } else if run.correctness["passed"] == true {
        "incomplete"
    } else {
        "failed"
    };
    if counting.is_object() && status != "running" && !complete_event {
        diagnostics.push("No accepted completion evidence exists for this counting window; missing counts are unknown.".into());
    }
    let mut counts = Vec::new();
    for row in rows.into_values() {
        if row["calls"].is_null() {
            diagnostics.push(format!(
                "Missing first-entry count for {}; resume segments are not substituted for calls",
                row["function"]["name"]
            ));
        } else {
            counts.push(row);
        }
    }
    let total = counts.len();
    let offset = params["cursor"]
        .as_str()
        .map(str::parse::<usize>)
        .transpose()
        .map_err(Failure::invalid_parameters)?
        .unwrap_or(0);
    let limit = params["limit"].as_u64().unwrap_or(100).clamp(1, 1000) as usize;
    let page = counts
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let next = offset.saturating_add(page.len());
    Ok(
        json!({"status":status,"run_id":run_id,"parent_run_id":counting["parent_run_id"],"method":method,"requested_scope":requested,"collection_scope":collection_scope,"counts":page,"total_count":total,"next_cursor":if next<total {Some(next.to_string())}else{None},"diagnostics":diagnostics,"limitations":limitations}),
    )
}

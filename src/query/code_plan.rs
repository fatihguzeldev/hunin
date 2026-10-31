//! Read-only investigation actions derived from one bounded recording snapshot.
use super::WorkBudget;
use crate::{model::runs::Run, storage::QueryReadPort};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn step(
    id: impl Into<String>,
    available: bool,
    reason: impl Into<String>,
    operation: Option<&str>,
    params: Value,
) -> Value {
    json!({"id":id.into(),"status":if available{"available"}else{"blocked"},"reason":reason.into(),"operation":operation,"params":params})
}

fn capture_request(run: &Run, goal: &str) -> Value {
    let mut command = run.command.clone();
    if let (Some(first), Some(executable)) = (
        command.first_mut(),
        run.source["launch_executable"].as_str(),
    ) {
        *first = executable.to_owned();
    }
    let mut options = serde_json::from_value::<crate::collection::CodeOptions>(
        run.source["capture_policy"]["code_options"].clone(),
    )
    .ok()
    .filter(|options| options.validate().is_ok())
    .unwrap_or_default();
    options.compiler_export = goal != "source";
    if !options.compiler_export {
        options.function_filter = None;
        options.wasm_function_index = None;
    }
    let mut params = json!({"name":format!("{} — code origins", run.name),"command":command,"runtime":"node","cwd":run.cwd,"timeout_seconds":30,"capture_mode":"code-origin", "code_options":options});
    if let Some(overrides) = run.source["environment"]["overrides"].as_object() {
        params["env"] = json!(overrides);
    }
    if let Some(recipe) = run.source["correctness_recipe"].as_object() {
        for key in [
            "expected_exit_code",
            "expected_stdout",
            "expected_stdout_sha256",
        ] {
            if let Some(value) = recipe.get(key).filter(|value| !value.is_null()) {
                params[key] = value.clone();
            }
        }
    }
    if let Some(files) = run.source["workload_inputs"]["files"].as_array() {
        params["input_files"] = json!(
            files
                .iter()
                .filter_map(|file| file["path"].as_str())
                .collect::<Vec<_>>()
        );
    }
    params
}

pub(super) fn plan(
    port: &mut QueryReadPort<'_>,
    params: &Value,
    work: &mut WorkBudget,
) -> Result<Value> {
    let inspection = super::code::inspect(port, params, work)?;
    let goal = params["goal"].as_str().unwrap_or("code_origin");
    let limit = params["limit"].as_u64().unwrap_or(100) as usize;
    let mut inspect_params = params.clone();
    inspect_params.as_object_mut().unwrap().remove("goal");
    if inspection["anchor"].is_null() {
        return Ok(
            json!({"anchor":null,"goal":goal,"steps":[step("missing-anchor",false,"This observation or frame is absent from the selected results.",None,json!({}))],"truncated":inspection["truncated"]}),
        );
    }
    let run = port.run()?;
    let mut steps = vec![step(
        "inspect-recorded-origins",
        true,
        "Inspect retained code instances, mapping evidence and explicitly scoped sample coverage.",
        Some("code.inspect"),
        inspect_params,
    )];
    let matching = inspection["input_artifacts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["status"] == "metadata_matches")
        .filter_map(|item| item["artifact_id"].as_str())
        .collect::<BTreeSet<_>>();
    let mut proofs = BTreeSet::new();
    for item in inspection["instances"].as_array().into_iter().flatten() {
        let instance = &item["instance"];
        if let Some(reference) = instance.get("artifact").filter(|value| !value.is_null())
            && let Some(id) = reference["artifact_id"]
                .as_str()
                .filter(|id| matching.contains(id))
        {
            work.entry()?;
            proofs.insert((
                id.to_owned(),
                reference["offset"].as_u64().unwrap_or(0),
                reference["length"].as_u64().unwrap_or(5000).clamp(1, 5000),
            ));
        }
    }
    for (collection, payload) in [("mappings", "mapping"), ("frame_mappings", "frame_mapping")] {
        for item in inspection[collection].as_array().into_iter().flatten() {
            for reference in item[payload]["proof"]["artifact_refs"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(id) = reference["artifact_id"]
                    .as_str()
                    .filter(|id| matching.contains(id))
                {
                    work.entry()?;
                    proofs.insert((
                        id.to_owned(),
                        reference["offset"].as_u64().unwrap_or(0),
                        reference["length"].as_u64().unwrap_or(5000).clamp(1, 5000),
                    ));
                }
            }
        }
    }
    for (id, offset, length) in proofs {
        work.entry()?;
        steps.push(step(format!("read-proof:{id}:{offset}:{length}"),true,"Read exact retained proof bytes as hexadecimal. Matching metadata does not itself establish semantic correctness.",Some("artifacts.read"),json!({"run_id":run.id,"artifact_id":id,"offset":offset,"limit":length,"encoding":"hex"})));
    }
    let artifacts = port.artifacts()?;
    let packages = port.analysis_packages(limit.saturating_add(1).min(501))?;
    let packages_truncated = packages.len() > limit;
    let mut declared_mapper = false;
    for package in packages.into_iter().take(limit) {
        work.entry()?;
        if package["output_contract"]["name"] != "code-mapping"
            || package["output_contract"]["version"] != 1
        {
            continue;
        }
        let Some(id) = package["id"].as_str() else {
            continue;
        };
        declared_mapper = true;
        let applicable =
            crate::analysis::applicability::check(&package["requirements"], &run, &artifacts);
        let name = package["name"].as_str().unwrap_or(id);
        let (available, reason) = match applicable {
            Ok(()) => (
                true,
                format!(
                    "Run {name}: its declared code-mapping contract and input metadata apply. Output evidence and applicability are rechecked at publication."
                ),
            ),
            Err(error) => (false, format!("{name} is not applicable: {error:#}")),
        };
        steps.push(step(
            format!("mapper:{id}"),
            available,
            reason,
            available.then_some("analyses.run"),
            if available {
                json!({"run_id":run.id,"analysis_id":id})
            } else {
                json!({})
            },
        ));
    }
    if !declared_mapper {
        let reason = if packages_truncated {
            "No code-mapping analyzer was found in the inspected package page. The package inventory is truncated; increase the limit to inspect more packages."
        } else {
            "No code-mapping analyzer is registered in this project. A source-aware mapper needs explicit input identities and evidence-backed output."
        };
        steps.push(step("mapper-unavailable", false, reason, None, json!({})));
    }
    work.check()?;
    if run.status == "running" {
        steps.push(step("capture-running",false,"This recording is still running. Finish or inspect it before planning another capture.",None,json!({})));
    } else if run.runtime != "node" || run.command.is_empty() {
        steps.push(step("code-capture-unavailable",false,"The built-in code-origin capture route currently requires an original Node launch command. Existing sampling and offline mapping remain available.",None,json!({})));
    } else {
        let capture = capture_request(&run, goal);
        // Retained launch metadata is not authority to execute a program during
        // a read. The explicit runs.start action validates current prerequisites.
        steps.push(step("capture-code-origins",true,"Request a separate opt-in recording with a 30-second timeout using the retained command and explicit inputs. Runtime prerequisites are checked at start; current source and inherited environment are recaptured. Compiler exports can change stdout and collection cost, and may remain partial. This cannot fill missing PCs in the old recording.",Some("runs.start"),capture));
    }
    let truncated = inspection["truncated"].as_bool().unwrap_or(false)
        || packages_truncated
        || steps.len() > limit;
    steps.truncate(limit);
    Ok(json!({"anchor":inspection["anchor"],"goal":goal,"steps":steps,"truncated":truncated}))
}

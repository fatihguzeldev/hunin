//! Foreground recording summaries; operations and retained wire data stay complete.
use crate::projects::Project;
use serde_json::{Value, json};

pub(super) fn recording(
    mut value: Value,
    project: Option<&Project>,
    selector: &[String],
    full: bool,
) -> Value {
    if full || !value["run"].is_object() {
        return value;
    }
    let run = &mut value["run"];
    let id = run["id"].as_str().unwrap_or("").to_owned();
    let source = run["source"].take();
    let mut compact = serde_json::Map::new();
    // Keep capture quality and failure/coverage evidence. Only deep provenance
    // and potentially thousands of individual source identities are omitted.
    for key in [
        "fingerprint",
        "revision",
        "dirty",
        "runtime_version",
        "build_source_verified",
        "source_snapshot_complete",
        "source_snapshot_scope",
        "source_retention",
        "collection_status",
        "collection_completion",
        "raw_retention",
        "sdk_ingestion",
        "resource_observation",
        "target_outcome",
        "collector_outcome",
        "native_session_cleanup",
        "supervised_pid",
        "supervised_elapsed_us",
        "supervised_execution_scope",
        "preparation",
        "diagnostic",
    ] {
        if let Some(value) = source.get(key) {
            compact.insert(key.into(), value.clone());
        }
    }
    if let Some(files) = source["source_files"].as_object() {
        compact.insert("source_files_count".into(), json!(files.len()));
    }
    if let Some(files) = source["retained_sources"].as_array() {
        compact.insert("retained_sources_count".into(), json!(files.len()));
    }
    run["source"] = json!(compact);
    value["format"] = json!("hunin-recording-summary-v1");
    value["project"] = project
        .map(|p| json!({"id":p.id,"name":p.name}))
        .unwrap_or(Value::Null);
    let mut inspect = vec!["hunin".to_owned()];
    inspect.extend_from_slice(selector);
    let mut hotspots = inspect.clone();
    inspect.extend(["inspect".into(), id.clone()]);
    hotspots.extend([
        "call".into(),
        "profiles.hotspots".into(),
        json!({"run_id":id,"limit":10}).to_string(),
    ]);
    value["next"] = json!({"inspect":inspect,"hotspots":hotspots,"full_output":"Use run --full, or execute inspect for complete recorded provenance. Generic call and all shared operation responses remain complete."});
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_failure_keeps_collector_failure_distinct_from_unknown_target_outcome() {
        let value = json!({"run":{"id":"failed-native","status":"failed","source":{
            "target_outcome":{"state":"unknown"},
            "collector_outcome":{"exit_code":73},
            "native_session_cleanup":{"acknowledged":false},
            "collection_status":"failed",
            "supervised_pid":42,
            "collector_settings":{"command":["collector"]}
        }}});
        let compact = recording(value, None, &[], false);
        let source = &compact["run"]["source"];
        assert_eq!(source["target_outcome"]["state"], "unknown");
        assert_eq!(source["collector_outcome"]["exit_code"], 73);
        assert_eq!(source["native_session_cleanup"]["acknowledged"], false);
        assert_eq!(source["collection_status"], "failed");
        assert_eq!(source["supervised_pid"], 42);
        assert!(source.get("collector_settings").is_none());
    }
}

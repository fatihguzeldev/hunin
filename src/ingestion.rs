use crate::model::relations::{ObservationLinks, Ownership, OwnershipKind, SelectedObservation};
use anyhow::{Result, bail};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

pub fn validate(run_id: &str, records: &[Value], existing: &HashSet<String>) -> Result<()> {
    static SCHEMA: OnceLock<jsonschema::Validator> = OnceLock::new();
    let schema = SCHEMA.get_or_init(|| {
        let source: Value =
            serde_json::from_str(include_str!("../protocol/record.schema.json")).unwrap();
        jsonschema::validator_for(&source).expect("embedded observation schema")
    });
    let mut ids = existing.clone();
    for record in records {
        if let Err(error) = schema.validate(record) {
            bail!("observation schema: {error}");
        }
        if record["run_id"].as_str() != Some(run_id) {
            bail!("observation belongs to another run");
        }
        let id = record["id"].as_str().unwrap();
        if !ids.insert(id.to_owned()) {
            bail!("duplicate observation id: {id}");
        }
    }
    for record in records {
        for field in ["parent_id", "context_id", "from", "to"] {
            if let Some(reference) = record[field].as_str() {
                if !ids.contains(reference) {
                    bail!("unresolved {field}: {reference}");
                }
                if reference == record["id"].as_str().unwrap() {
                    bail!("self reference: {reference}");
                }
            }
        }
        for reference in record["evidence"].as_array().into_iter().flatten() {
            if !ids.contains(reference.as_str().unwrap()) {
                bail!("unresolved evidence: {reference}");
            }
        }
        if record["kind"] == "sample" && record["basis"] != "sampled" {
            bail!("CPU samples must retain sampled basis");
        }
    }
    Ok(())
}

pub fn validate_ownership(records: &[Value], existing: &HashMap<String, Value>) -> Result<()> {
    let selected = existing
        .iter()
        .map(|(id, row)| {
            (
                id.clone(),
                SelectedObservation {
                    ownership: ownership(row),
                    result_set_id: String::new(),
                },
            )
        })
        .collect();
    crate::publication::validate_ownership(&links(records), &selected)
}

fn ownership(record: &Value) -> Ownership {
    Ownership {
        kind: OwnershipKind::from_wire(record["kind"].as_str().unwrap_or("")),
        parent_id: record["parent_id"].as_str().map(str::to_owned),
        context_id: record["context_id"].as_str().map(str::to_owned),
    }
}

/// Called after schema validation. This adapter owns JSON field interpretation;
/// publication reasons only about typed links and selected result identities.
pub(crate) fn links(records: &[Value]) -> Vec<ObservationLinks> {
    records
        .iter()
        .map(|record| ObservationLinks {
            id: record["id"].as_str().unwrap().to_owned(),
            ownership: ownership(record),
            references: ["parent_id", "context_id", "from", "to"]
                .into_iter()
                .filter_map(|field| record[field].as_str())
                .chain(
                    record["evidence"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str),
                )
                .map(str::to_owned)
                .collect(),
        })
        .collect()
}

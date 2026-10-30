use crate::model::identity::{ObservationId, ResultId};
use crate::model::relations::{ObservationLinks, Ownership, OwnershipKind, SelectedObservation};
use anyhow::{Result, bail};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

pub fn validate(run_id: &str, records: &[Value], existing: &HashSet<String>) -> Result<()> {
    let existing = existing.iter().cloned().map(ObservationId::from).collect();
    validate_ids(run_id, records, existing)
}

pub(crate) fn validate_ids(
    run_id: &str,
    records: &[Value],
    mut ids: HashSet<ObservationId>,
) -> Result<()> {
    static SCHEMA: OnceLock<jsonschema::Validator> = OnceLock::new();
    let schema = SCHEMA.get_or_init(|| {
        let source: Value =
            serde_json::from_str(include_str!("../protocol/record.schema.json")).unwrap();
        jsonschema::validator_for(&source).expect("embedded observation schema")
    });
    for record in records {
        if let Err(error) = schema.validate(record) {
            bail!("observation schema: {error}");
        }
        for field in ["timestamp_us", "duration_us", "pid"] {
            if let Some(value) = record.get(field) {
                anyhow::ensure!(
                    crate::model::numbers::unsigned_integer(value).is_some(),
                    "observation {field} must be an exactly representable unsigned integer"
                );
            }
        }
        for frame in record["stack"].as_array().into_iter().flatten() {
            for field in ["line", "column"] {
                if let Some(value) = frame.get(field) {
                    anyhow::ensure!(
                        crate::model::numbers::unsigned_integer(value).is_some(),
                        "observation frame {field} must be an exactly representable unsigned integer"
                    );
                }
            }
        }
        if record["run_id"].as_str() != Some(run_id) {
            bail!("observation belongs to another run");
        }
        let id = record["id"].as_str().unwrap();
        if !ids.insert(ObservationId::from(id)) {
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
                ObservationId::from(id.clone()),
                SelectedObservation {
                    ownership: ownership(row),
                    result_set_id: ResultId::from(""),
                },
            )
        })
        .collect();
    crate::publication::validate_ownership(&links(records), &selected)
}

fn ownership(record: &Value) -> Ownership {
    Ownership {
        kind: OwnershipKind::from_wire(record["kind"].as_str().unwrap_or("")),
        parent_id: record["parent_id"].as_str().map(ObservationId::from),
        context_id: record["context_id"].as_str().map(ObservationId::from),
    }
}

/// Called after schema validation. This adapter owns JSON field interpretation;
/// publication reasons only about typed links and selected result identities.
pub(crate) fn links(records: &[Value]) -> Vec<ObservationLinks> {
    records
        .iter()
        .map(|record| ObservationLinks {
            id: ObservationId::from(record["id"].as_str().unwrap()),
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
                .map(ObservationId::from)
                .collect(),
        })
        .collect()
}

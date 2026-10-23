use anyhow::{Result, bail};
use serde_json::Value;
use std::collections::HashSet;
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

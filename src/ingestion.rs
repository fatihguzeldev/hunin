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
    let mut all = existing.clone();
    all.extend(
        records
            .iter()
            .map(|r| (r["id"].as_str().unwrap().to_owned(), r.clone())),
    );
    for record in records {
        for (field, kind) in [("parent_id", "span"), ("context_id", "context")] {
            if let Some(id) = record[field].as_str()
                && all.get(id).is_none_or(|r| r["kind"] != kind)
            {
                bail!("{field} must reference a {kind}: {id}");
            }
        }
        for field in ["parent_id", "context_id"] {
            let mut seen = HashSet::new();
            let mut current = Some(record);
            while let Some(row) = current {
                if !seen.insert(row["id"].as_str().unwrap()) {
                    bail!("cyclic {field} ownership");
                }
                current = row[field].as_str().and_then(|id| all.get(id));
            }
        }
    }
    Ok(())
}

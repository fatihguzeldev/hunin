use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

const RESPONSE_REFERENCE: &str = "https://hunin.dev/protocol/response-v1.schema.json#/$defs/";

/// Bundle only reachable definitions so exported MCP output schemas work offline.
pub(super) fn bundle(schema: &Value, definitions: &Map<String, Value>) -> Result<Value> {
    let mut schema = schema.clone();
    let mut pending = BTreeSet::new();
    localize(&mut schema, &mut pending)?;
    let mut included = Map::new();
    while let Some(name) = pending.pop_first() {
        if included.contains_key(&name) {
            continue;
        }
        let mut definition = definitions
            .get(&name)
            .with_context(|| format!("unknown response definition: {name}"))?
            .clone();
        localize(&mut definition, &mut pending)?;
        included.insert(name, definition);
    }
    if !included.is_empty() {
        schema["$defs"] = Value::Object(included);
    }
    Ok(schema)
}

fn localize(value: &mut Value, pending: &mut BTreeSet<String>) -> Result<()> {
    match value {
        Value::Object(fields) => {
            if let Some(reference) = fields.get_mut("$ref") {
                let text = reference
                    .as_str()
                    .context("response reference must be a string")?;
                let Some(name) = text
                    .strip_prefix(RESPONSE_REFERENCE)
                    .or_else(|| text.strip_prefix("#/$defs/"))
                else {
                    bail!("unsupported response schema reference: {text}");
                };
                pending.insert(name.to_owned());
                *reference = Value::String(format!("#/$defs/{name}"));
            }
            for field in fields.values_mut() {
                localize(field, pending)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                localize(item, pending)?;
            }
        }
        _ => {}
    }
    Ok(())
}

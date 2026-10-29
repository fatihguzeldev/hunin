use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::{collections::BTreeSet, sync::OnceLock};

const RESPONSE_REFERENCE: &str = "https://hunin.dev/protocol/response-v1.schema.json#/$defs/";

/// Bundle only reachable definitions so exported MCP output schemas work offline.
pub(super) fn bundle(schema: &Value, definitions: &Map<String, Value>) -> Result<Value> {
    if let Some(fields) = schema.as_object()
        && fields.contains_key("$ref")
        && fields.len() != 1
    {
        bail!(
            "MCP output root references cannot have siblings; define the complete object in $defs"
        );
    }
    let mut schema = schema.clone();
    let mut pending = BTreeSet::new();
    localize(&mut schema, &mut pending)?;
    let mut included = Map::new();
    while let Some(name) = pending.pop_first() {
        if included.contains_key(&name) {
            continue;
        }
        let definition = definitions
            .get(&name)
            .with_context(|| format!("unknown response definition: {name}"))?;
        let mut definition =
            if definition["$ref"] == "https://hunin.dev/protocol/record-v1.schema.json" {
                static RECORD: OnceLock<Value> = OnceLock::new();
                RECORD
                    .get_or_init(|| {
                        serde_json::from_str(include_str!("../../protocol/record.schema.json"))
                            .expect("embedded observation schema")
                    })
                    .clone()
            } else {
                definition.clone()
            };
        localize(&mut definition, &mut pending)?;
        included.insert(name, definition);
    }
    if schema.get("type").is_none() {
        let root = schema["$ref"]
            .as_str()
            .and_then(|reference| reference.strip_prefix("#/$defs/"))
            .and_then(|name| included.get(name));
        if let Some(root) = root.filter(|root| root["type"] == "object") {
            schema = root.clone();
        }
    }
    if schema["type"] != "object" {
        bail!("MCP operation output schemas must declare an object root");
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

#[cfg(test)]
mod tests {
    use super::bundle;
    use serde_json::json;

    #[test]
    fn root_reference_siblings_cannot_be_silently_discarded() {
        let definitions = json!({"Example": {"type": "object"}});
        for sibling in [
            json!({"minProperties": 1}),
            json!({"description": "retained annotation"}),
        ] {
            let mut schema = sibling;
            schema["$ref"] = json!("#/$defs/Example");
            let error = bundle(&schema, definitions.as_object().unwrap()).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("root references cannot have siblings")
            );
        }
        let schema = bundle(
            &json!({"$ref": "#/$defs/Example"}),
            definitions.as_object().unwrap(),
        )
        .unwrap();
        assert_eq!(schema["type"], "object");
    }
}

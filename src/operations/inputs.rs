use serde_json::Value;

/// JSON Schema integers include 2.0. Adapt representable integer fields before
/// typed consumers read them; number fields and opaque JSON keep their form.
pub(super) fn canonicalize_integers(schema: &Value, value: &mut Value) {
    if schema["type"] == "integer" {
        if value.as_u64().is_some() || value.as_i64().is_some() {
            return;
        }
        if let Some(number) = value.as_f64().filter(|number| number.fract() == 0.0) {
            // u64::MAX rounds to the exclusive 2^64 boundary as f64.
            if (0.0..u64::MAX as f64).contains(&number) {
                *value = Value::from(number as u64);
            } else if (i64::MIN as f64..0.0).contains(&number) {
                *value = Value::from(number as i64);
            }
        }
        return;
    }
    if let (Some(properties), Some(fields)) =
        (schema["properties"].as_object(), value.as_object_mut())
    {
        for (key, schema) in properties {
            if let Some(value) = fields.get_mut(key) {
                canonicalize_integers(schema, value);
            }
        }
    } else if let (Some(schema), Some(items)) = (schema.get("items"), value.as_array_mut()) {
        for value in items {
            canonicalize_integers(schema, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_declared_integer_fields_change_numeric_representation() {
        let schema = json!({"type":"object","properties":{"limit":{"type":"integer"},"signed":{"type":"integer"},"duration":{"type":"number"},"opaque":{}}});
        let mut value = json!({"limit":2.0,"signed":-3.0,"duration":2.0,"opaque":{"nested":2.0,"id":"18446744073709551615"}});
        canonicalize_integers(&schema, &mut value);
        assert_eq!(value["limit"].as_u64(), Some(2));
        assert_eq!(value["signed"].as_i64(), Some(-3));
        assert!(value["duration"].as_number().unwrap().is_f64());
        assert!(value["opaque"]["nested"].as_number().unwrap().is_f64());
        assert_eq!(value["opaque"]["id"], "18446744073709551615");
    }
}

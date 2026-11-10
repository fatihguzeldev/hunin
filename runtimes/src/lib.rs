//! source-specific runtime observation and extraction.

pub mod v8;

use hunin_core::records::{Clock, FieldSchema, FieldType, Record, RecordSchema};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_input_bytes: usize,
    pub max_records: usize,
    pub max_output_bytes: usize,
    pub max_string_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: 16 * 1024 * 1024,
            max_records: 100_000,
            max_output_bytes: 64 * 1024 * 1024,
            max_string_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Extraction {
    /// producer export completeness within its declared scope; resolution diagnostics are separate.
    pub capture_complete: bool,
    pub schemas: Vec<RecordSchema>,
    pub clocks: Vec<Clock>,
    pub records: Vec<Record>,
    pub metadata: Value,
    pub warnings: Vec<String>,
    output_bytes: usize,
    limits: Limits,
}

impl Extraction {
    fn new(limits: Limits, metadata: Value) -> Self {
        Self {
            capture_complete: true,
            schemas: vec![],
            clocks: vec![],
            records: vec![],
            metadata,
            warnings: vec![],
            output_bytes: 0,
            limits,
        }
    }

    fn push(&mut self, record: Record) -> Result<()> {
        if self.records.len() >= self.limits.max_records {
            return Err(Error("extracted record budget exceeded".into()));
        }
        let mut pending = vec![&record.payload];
        while let Some(value) = pending.pop() {
            match value {
                Value::String(text) if text.len() > self.limits.max_string_bytes => {
                    return Err(Error("source string budget exceeded".into()));
                }
                Value::Object(fields) => {
                    if fields
                        .keys()
                        .any(|key| key.len() > self.limits.max_string_bytes)
                    {
                        return Err(Error("source key budget exceeded".into()));
                    }
                    pending.extend(fields.values());
                }
                Value::Array(values) => pending.extend(values),
                _ => {}
            }
        }
        let bytes = serde_json::to_vec(&record)
            .map_err(|e| Error(e.to_string()))?
            .len();
        if bytes > 1024 * 1024 {
            return Err(Error("extracted record exceeds core's 1 MiB limit".into()));
        }
        self.output_bytes = self
            .output_bytes
            .checked_add(bytes)
            .ok_or_else(|| Error("output byte count overflow".into()))?;
        if self.output_bytes > self.limits.max_output_bytes {
            return Err(Error("extracted byte budget exceeded".into()));
        }
        self.records.push(record);
        Ok(())
    }
}

fn input(bytes: &[u8], limits: Limits) -> Result<Map<String, Value>> {
    if bytes.len() > limits.max_input_bytes {
        return Err(Error("source input byte budget exceeded".into()));
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|e| Error(e.to_string()))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| Error("source document must be an object".into()))
}

fn array<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a Vec<Value>> {
    object
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| Error(format!("{key} must be an array")))
}
fn unsigned(object: &Map<String, Value>, key: &str) -> Result<u64> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error(format!("{key} must be an unsigned integer")))
}
fn text<'a>(object: &'a Map<String, Value>, key: &str, limits: Limits) -> Result<&'a str> {
    let text = object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error(format!("{key} must be a string")))?;
    if text.len() > limits.max_string_bytes {
        return Err(Error(format!("{key} exceeds string budget")));
    }
    Ok(text)
}
fn object(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| Error("expected an object record".into()))
}
fn schema(
    name: &str,
    description: &str,
    fields: &[(&str, FieldType, Option<&str>, &str)],
) -> RecordSchema {
    RecordSchema {
        name: name.into(),
        version: 1,
        description: description.into(),
        fields: fields
            .iter()
            .map(|(name, kind, unit, meaning)| {
                (
                    (*name).into(),
                    FieldSchema {
                        kind: kind.clone(),
                        required: true,
                        unit: unit.map(str::to_string),
                        meaning: (*meaning).into(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>(),
    }
}
fn record(id: String, schema: &str, subject: &str, payload: Value) -> Record {
    Record {
        id,
        schema: schema.into(),
        schema_version: 1,
        subject: subject.into(),
        time: None,
        function: None,
        location: None,
        operation: None,
        payload,
    }
}

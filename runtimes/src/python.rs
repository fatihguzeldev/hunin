use crate::{
    Error, Extraction, Limits, Result, array, input, object, record, schema, text, unsigned,
};
use hunin_core::records::FieldType;
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub fn extract(bytes: &[u8], subject: &str, limits: Limits) -> Result<Extraction> {
    let mut root = input(bytes, limits)?;
    let runtime = text(&root, "runtime", limits)?;
    if !["cpython", "pypy"].contains(&runtime) {
        return Err(Error("unsupported python profiler runtime".into()));
    }
    text(&root, "version", limits)?;
    let functions = array(&root, "functions")?;
    let mut identities = BTreeSet::new();
    for function in functions {
        let function = object(function)?;
        if !identities.insert(unsigned(function, "id")?) {
            return Err(Error("duplicate python function identity".into()));
        }
    }
    let mut extracted = Extraction::new(
        limits,
        json!({"format":"python.cprofile","time_meaning":"profiler elapsed seconds; aggregate, not process cpu or per-invocation timestamps"}),
    );
    extracted.schemas = schemas();
    let mut unresolved_edges = 0u64;
    let mut unresolved_calls = 0u64;
    for function in functions {
        let mut payload = object(function)?.clone();
        let id = unsigned(&payload, "id")?;
        let name = text(&payload, "name", limits)?.to_string();
        costs(&payload)?;
        let callees = payload
            .remove("callees")
            .ok_or_else(|| Error("missing caller/callee coverage".into()))?;
        let mut function = record(
            format!("function:{id}"),
            "python.cprofile-function",
            subject,
            Value::Object(payload),
        );
        function.function = Some(name);
        if let (Some(file), Some(line)) = (
            function.payload["file"].as_str(),
            function.payload["line"].as_u64(),
        ) {
            if file.len() > limits.max_string_bytes {
                return Err(Error("python filename exceeds budget".into()));
            }
            function.location = Some(format!("{file}:{line}"));
        }
        extracted.push(function)?;
        for (ordinal, callee) in callees
            .as_array()
            .ok_or_else(|| Error("callees must be an array".into()))?
            .iter()
            .enumerate()
        {
            let mut callee = object(callee)?.clone();
            costs(&callee)?;
            if let Some(target) = callee.get("callee").and_then(Value::as_u64) {
                if !identities.contains(&target) {
                    return Err(Error("unresolved python callee identity".into()));
                }
            } else {
                unresolved_edges += 1;
                unresolved_calls = unresolved_calls
                    .checked_add(unsigned(&callee, "calls")?)
                    .ok_or_else(|| crate::Error("unresolved callee call count overflow".into()))?;
            }
            callee.insert("caller".into(), json!(id));
            extracted.push(record(
                format!("edge:{id}:{ordinal}"),
                "python.cprofile-edge",
                subject,
                Value::Object(callee),
            ))?;
        }
    }
    extracted.metadata["callee_resolution"] = json!({"unresolved_edges":unresolved_edges,"reported_calls_on_unresolved_edges":unresolved_calls,"meaning":"callee_code and per-edge statistics retained; missing aggregate identity does not imply dropped records"});
    if unresolved_edges > 0 {
        extracted.warnings.push(format!("{unresolved_edges} callee aggregate identities unavailable ({unresolved_calls} reported calls); code and edge statistics retained"));
    }
    root.remove("functions");
    extracted.push(record(
        "profile".into(),
        "python.cprofile",
        subject,
        Value::Object(root),
    ))?;
    Ok(extracted)
}

fn costs(value: &serde_json::Map<String, Value>) -> Result<()> {
    unsigned(value, "calls")?;
    unsigned(value, "recursive_calls")?;
    for name in ["total_seconds", "self_seconds"] {
        let number = value
            .get(name)
            .and_then(Value::as_f64)
            .ok_or_else(|| Error(format!("invalid {name}")))?;
        if !number.is_finite() || number < 0.0 {
            return Err(Error(format!("invalid {name}")));
        }
    }
    Ok(())
}

pub fn schemas() -> Vec<hunin_core::records::RecordSchema> {
    vec![
        schema(
            "python.cprofile",
            "profiler runtime and coverage metadata",
            &[
                ("runtime", FieldType::String, None, "python implementation"),
                (
                    "version",
                    FieldType::String,
                    None,
                    "actual interpreter version",
                ),
            ],
        ),
        schema(
            "python.cprofile-function",
            "deterministic function aggregates",
            &[
                (
                    "id",
                    FieldType::Unsigned,
                    None,
                    "source-local function identity",
                ),
                (
                    "name",
                    FieldType::String,
                    None,
                    "code-object or builtin name",
                ),
                (
                    "file",
                    FieldType::Nullable(Box::new(FieldType::String)),
                    None,
                    "python source path; null for builtin entries",
                ),
                (
                    "line",
                    FieldType::Nullable(Box::new(FieldType::Unsigned)),
                    None,
                    "code declaration line; null when unavailable",
                ),
                (
                    "calls",
                    FieldType::Unsigned,
                    Some("calls"),
                    "total call count",
                ),
                (
                    "recursive_calls",
                    FieldType::Unsigned,
                    Some("calls"),
                    "recursive calls",
                ),
                (
                    "total_seconds",
                    FieldType::Float,
                    Some("seconds"),
                    "inclusive profiler elapsed time",
                ),
                (
                    "self_seconds",
                    FieldType::Float,
                    Some("seconds"),
                    "exclusive profiler elapsed time",
                ),
            ],
        ),
        schema(
            "python.cprofile-edge",
            "observed caller/callee aggregates",
            &[
                ("caller", FieldType::Unsigned, None, "caller function id"),
                (
                    "callee",
                    FieldType::Nullable(Box::new(FieldType::Unsigned)),
                    None,
                    "callee function id, null for unresolved or ambiguous source identities",
                ),
                (
                    "calls",
                    FieldType::Unsigned,
                    Some("calls"),
                    "observed edge call count",
                ),
                (
                    "recursive_calls",
                    FieldType::Unsigned,
                    Some("calls"),
                    "recursive edge calls",
                ),
                (
                    "total_seconds",
                    FieldType::Float,
                    Some("seconds"),
                    "inclusive elapsed on this edge",
                ),
                (
                    "self_seconds",
                    FieldType::Float,
                    Some("seconds"),
                    "exclusive elapsed on this edge",
                ),
            ],
        ),
    ]
}

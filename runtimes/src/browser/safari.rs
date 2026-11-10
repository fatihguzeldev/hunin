use crate::{
    Error, Extraction, Limits, Result, array, input, object, record, schema, text, unsigned,
};
use hunin_core::records::{Clock, FieldType, RecordSchema, RecordTime};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
    time::Duration,
};

/// compiles caller-prepared javascript with the explicitly selected Babel toolchain.
pub fn instrument(
    node: &Path,
    babel_root: &Path,
    source: &Path,
    output: &Path,
    binding: &Path,
    area: &Path,
) -> Result<Value> {
    if ![node, babel_root, source, output, binding, area]
        .iter()
        .all(|path| path.is_absolute())
    {
        return Err(Error(
            "Safari instrumentation paths must be absolute".into(),
        ));
    }
    if !fs::symlink_metadata(area)
        .map_err(|e| Error(e.to_string()))?
        .file_type()
        .is_dir()
    {
        return Err(Error(
            "instrumentation area must be an owned directory".into(),
        ));
    }
    let area = fs::canonicalize(area).map_err(|e| Error(e.to_string()))?;
    for path in [output, binding] {
        if fs::canonicalize(
            path.parent()
                .ok_or_else(|| Error("missing output parent".into()))?,
        )
        .map_err(|e| Error(e.to_string()))?
            != area
        {
            return Err(Error(
                "instrumented outputs must be direct children of their owned area".into(),
            ));
        }
    }
    let helper = area.join("safari-instrument.mjs");
    if helper.exists() {
        if crate::prepared::bounded_file(&helper, 64 * 1024)?
            != include_bytes!("../../helpers/safari-instrument.mjs")
        {
            return Err(Error(
                "owned Safari compiler helper differs from selected implementation".into(),
            ));
        }
    } else {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&helper)
            .map_err(|e| Error(e.to_string()))?;
        file.write_all(include_str!("../../helpers/safari-instrument.mjs").as_bytes())
            .map_err(|e| Error(e.to_string()))?;
    }
    let mut command = Command::new(node);
    command.args([helper.as_path(), source, output, binding, babel_root]);
    let result =
        hunin_platforms::tools::run(&mut command, &area, Duration::from_secs(30), 64 * 1024)
            .map_err(|e| Error(e.to_string()))?;
    hunin_platforms::tools::require_success(result).map_err(|e| Error(e.to_string()))?;
    let bytes = crate::prepared::bounded_file(binding, 4 * 1024 * 1024)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(Error("Safari binding byte budget exceeded".into()));
    }
    let metadata: Value = serde_json::from_slice(&bytes).map_err(|e| Error(e.to_string()))?;
    let instrumented = crate::prepared::bounded_file(output, 16 * 1024 * 1024)?;
    if metadata["instrumented_sha256"] != crate::source_maps::digest(&instrumented) {
        return Err(Error(
            "instrumented Safari bytes do not match compiler binding".into(),
        ));
    }
    Ok(metadata)
}
pub fn clocks() -> Vec<Clock> {
    vec![Clock{id:"safari-instrumented-page".into(),ticks_per_second:1_000_000,description:"page performance.now milliseconds projected to integer microseconds; conversion rounding is separate from browser precision/privacy quantisation; no OS clock alignment".into()}]
}
pub fn schemas() -> Vec<RecordSchema> {
    vec![
        schema(
            "safari.instrumentation",
            "compiler-inserted function observation scope and source-reported loss",
            &[
                (
                    "format",
                    FieldType::String,
                    None,
                    "instrumentation export version",
                ),
                (
                    "lost_events",
                    FieldType::Unsigned,
                    Some("events"),
                    "known hook events dropped at the configured buffer limit",
                ),
                (
                    "hook_errors",
                    FieldType::Unsigned,
                    Some("errors"),
                    "observer failures swallowed to preserve application completion",
                ),
                (
                    "open_calls",
                    FieldType::Array,
                    None,
                    "body activations still open at stop; async/generator suspension included",
                ),
                (
                    "time_origin_ms",
                    FieldType::Float,
                    Some("ms"),
                    "raw page performance.timeOrigin; no OS clock alignment",
                ),
                (
                    "start_ms",
                    FieldType::Float,
                    Some("ms"),
                    "raw page performance.now at capture start",
                ),
                (
                    "end_ms",
                    FieldType::Float,
                    Some("ms"),
                    "raw page performance.now at capture stop",
                ),
                (
                    "first_loss_ms",
                    FieldType::Nullable(Box::new(FieldType::Float)),
                    Some("ms"),
                    "raw page clock of first known dropped event, or null",
                ),
            ],
        ),
        schema(
            "safari.script",
            "explicit compiler/driver script byte binding",
            &[
                (
                    "id",
                    FieldType::String,
                    None,
                    "instrumentation-local script registration",
                ),
                (
                    "source_name",
                    FieldType::String,
                    None,
                    "original input script identity",
                ),
                (
                    "original_sha256",
                    FieldType::String,
                    None,
                    "original input byte hash",
                ),
                (
                    "instrumented_sha256",
                    FieldType::String,
                    None,
                    "bound executed instrumented byte hash",
                ),
                (
                    "transform_id",
                    FieldType::String,
                    None,
                    "compiler transform identity",
                ),
            ],
        ),
        schema(
            "safari.function",
            "compiler source declaration; does not assert execution",
            &[
                (
                    "id",
                    FieldType::String,
                    None,
                    "source-local declaration identity",
                ),
                (
                    "script",
                    FieldType::String,
                    None,
                    "script registration identity",
                ),
                (
                    "name",
                    FieldType::String,
                    None,
                    "static source label; computed names may be unresolved",
                ),
                (
                    "line",
                    FieldType::Unsigned,
                    None,
                    "zero-based original input declaration line",
                ),
                (
                    "column",
                    FieldType::Unsigned,
                    None,
                    "zero-based original input UTF16 column",
                ),
                ("async", FieldType::Boolean, None, "async function body"),
                ("generator", FieldType::Boolean, None, "generator body"),
                (
                    "ordinal",
                    FieldType::Unsigned,
                    None,
                    "compiler-local declaration ordinal",
                ),
                (
                    "kind",
                    FieldType::String,
                    None,
                    "Babel source declaration kind",
                ),
                (
                    "end_line",
                    FieldType::Unsigned,
                    None,
                    "zero-based original declaration end line",
                ),
                (
                    "end_column",
                    FieldType::Unsigned,
                    None,
                    "zero-based original declaration end UTF16 column",
                ),
            ],
        ),
        schema(
            "safari.call",
            "one compiler-inserted body entry or completion event",
            &[
                (
                    "call_id",
                    FieldType::Unsigned,
                    None,
                    "one body activation; no inferred parent",
                ),
                (
                    "function_id",
                    FieldType::String,
                    None,
                    "observed declaration id",
                ),
                ("kind", FieldType::String, None, "entry or exit"),
                (
                    "time_ms",
                    FieldType::Float,
                    Some("ms"),
                    "raw page performance.now callback time",
                ),
                (
                    "body_throw",
                    FieldType::Nullable(Box::new(FieldType::Boolean)),
                    None,
                    "body completion threw; promise settlement is a separate event not observed here",
                ),
            ],
        ),
    ]
}
fn hash(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}
pub fn extract(bytes: &[u8], subject: &str, limits: Limits) -> Result<Extraction> {
    let mut header = input(bytes, limits)?;
    if text(&header, "format", limits)? != "hunin.safari-instrument-v1" {
        return Err(Error("unsupported Safari instrumentation export".into()));
    }
    let time_value = |key: &str| {
        header
            .get(key)
            .and_then(Value::as_f64)
            .filter(|time| time.is_finite() && *time >= 0.0)
            .ok_or_else(|| Error(format!("invalid Safari header clock {key}")))
    };
    let start = time_value("start_ms")?;
    let end = time_value("end_ms")?;
    time_value("time_origin_ms")?;
    if end < start {
        return Err(Error("Safari capture end precedes start".into()));
    }
    let mut out = Extraction::new(
        limits,
        json!({"origin":"instrumented","method":"Babel8.1 generic source function body hooks","meaning":"observed body wall spans, including suspension and observer overhead; no inferred async parent and no CPU ownership"}),
    );
    out.schemas = schemas();
    out.clocks = clocks();
    let mut scripts = BTreeMap::new();
    for value in array(&header, "scripts")? {
        let item = object(value)?;
        let id = text(item, "id", limits)?;
        text(item, "source_name", limits)?;
        for field in ["original_sha256", "instrumented_sha256", "transform_id"] {
            if !item.get(field).is_some_and(hash) {
                return Err(Error(format!(
                    "missing valid Safari script binding {field}"
                )));
            }
        }
        if scripts.insert(id.to_string(), value.clone()).is_some() {
            return Err(Error("duplicate script registration".into()));
        }
        out.push(record(
            format!("script:{id}"),
            "safari.script",
            subject,
            value.clone(),
        ))?;
    }
    let mut functions = BTreeMap::new();
    for value in array(&header, "functions")? {
        let item = object(value)?;
        let id = text(item, "id", limits)?;
        let script = text(item, "script", limits)?;
        if !scripts.contains_key(script) {
            return Err(Error("function refers to missing Safari script".into()));
        }
        let name = text(item, "name", limits)?;
        unsigned(item, "ordinal")?;
        let line = unsigned(item, "line")?;
        let column = unsigned(item, "column")?;
        let end_line = unsigned(item, "end_line")?;
        let end_column = unsigned(item, "end_column")?;
        if (end_line, end_column) < (line, column) {
            return Err(Error("reversed Safari declaration span".into()));
        }
        let kind = text(item, "kind", limits)?;
        if ![
            "FunctionDeclaration",
            "FunctionExpression",
            "ArrowFunctionExpression",
            "ObjectMethod",
            "ClassMethod",
            "ClassPrivateMethod",
        ]
        .contains(&kind)
        {
            return Err(Error("unsupported Safari declaration kind".into()));
        }
        if ["async", "generator"]
            .iter()
            .any(|key| !item.get(*key).is_some_and(Value::is_boolean))
        {
            return Err(Error(
                "Safari declaration async/generator fields must be booleans".into(),
            ));
        }
        if functions
            .insert(
                id.to_string(),
                (
                    name.to_string(),
                    format!(
                        "{}:{}:{}",
                        scripts[script]["source_name"].as_str().unwrap(),
                        line.checked_add(1)
                            .ok_or_else(|| Error("Safari source line overflow".into()))?,
                        column
                            .checked_add(1)
                            .ok_or_else(|| Error("Safari source column overflow".into()))?
                    ),
                ),
            )
            .is_some()
        {
            return Err(Error("duplicate Safari declaration".into()));
        }
        let mut row = record(
            format!("function:{id}"),
            "safari.function",
            subject,
            value.clone(),
        );
        row.function = Some(name.to_string());
        row.location = Some(functions[id].1.clone());
        out.push(row)?;
    }
    let mut active = BTreeMap::new();
    let mut calls = BTreeSet::new();
    let mut last_time = None;
    for (ordinal, value) in array(&header, "events")?.iter().enumerate() {
        let item = object(value)?;
        let id = unsigned(item, "call_id")?;
        let function = text(item, "function_id", limits)?;
        let Some((name, location)) = functions.get(function) else {
            return Err(Error("event refers to missing Safari declaration".into()));
        };
        let kind = text(item, "kind", limits)?;
        let time = item
            .get("time_ms")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && *n >= 0.0)
            .ok_or_else(|| Error("invalid Safari event clock".into()))?;
        if last_time.is_some_and(|previous| time < previous) {
            return Err(Error("Safari event clock regressed".into()));
        }
        last_time = Some(time);
        if time < start || time > end {
            return Err(Error("Safari event is outside the source window".into()));
        }
        match kind {
            "entry" => {
                if !item.get("body_throw").is_some_and(Value::is_null) {
                    return Err(Error("Safari body entry has no completion outcome".into()));
                }
                if id == 0 || !calls.insert(id) || active.insert(id, function.to_string()).is_some()
                {
                    return Err(Error("duplicate Safari activation".into()));
                }
            }
            "exit" => {
                if !item.get("body_throw").is_some_and(Value::is_boolean) {
                    return Err(Error(
                        "Safari body completion needs a boolean throw outcome".into(),
                    ));
                }
                if active.remove(&id).as_deref() != Some(function) {
                    return Err(Error("unmatched Safari body completion".into()));
                }
            }
            _ => return Err(Error("unknown Safari call event".into())),
        }
        let mut row = record(
            format!("event:{ordinal}"),
            "safari.call",
            subject,
            value.clone(),
        );
        row.function = Some(name.clone());
        row.location = Some(location.clone());
        let micros = (time * 1000.0).floor();
        if micros < i64::MAX as f64 {
            row.time = Some(RecordTime {
                clock: "safari-instrumented-page".into(),
                start: micros as i64,
                end: None,
                uncertainty: 1,
            });
        }
        out.push(row)?;
    }
    let lost = unsigned(&header, "lost_events")?;
    let errors = unsigned(&header, "hook_errors")?;
    object(
        header
            .get("semantics")
            .ok_or_else(|| Error("missing Safari observation semantics".into()))?,
    )?;
    match header.get("first_loss_ms") {
        Some(Value::Null) if lost == 0 => {}
        Some(value)
            if lost > 0
                && value
                    .as_f64()
                    .is_some_and(|time| time.is_finite() && time >= start && time <= end) => {}
        _ => {
            return Err(Error(
                "Safari first-loss clock disagrees with event-loss summary".into(),
            ));
        }
    }
    if lost > 0 || errors > 0 {
        out.capture_complete = false;
        out.warnings.push(format!(
            "instrumentation lost {lost} events and reported {errors} hook errors"
        ));
    }
    let open_values = array(&header, "open_calls")?;
    let open = open_values
        .iter()
        .map(|v| {
            v.as_u64()
                .ok_or_else(|| Error("invalid open activation id".into()))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    if open.len() != open_values.len() || open.contains(&0) {
        return Err(Error("duplicate or zero Safari open activation".into()));
    }
    if errors == 0 && open.iter().any(|id| !active.contains_key(id)) {
        return Err(Error("Safari open activation has no retained entry".into()));
    }
    if lost == 0 && errors == 0 && open != active.keys().copied().collect() {
        return Err(Error(
            "Safari open activation summary does not match retained events".into(),
        ));
    }
    for key in ["scripts", "functions", "events"] {
        header.remove(key);
    }
    out.push(record(
        "instrumentation".into(),
        "safari.instrumentation",
        subject,
        Value::Object(header),
    ))?;
    Ok(out)
}

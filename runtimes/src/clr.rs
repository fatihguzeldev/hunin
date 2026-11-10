use crate::{
    Error, Extraction, Limits, Result, array, input, object, record, schema, text, unsigned,
};
use hunin_core::records::{Clock, FieldType, RecordSchema, RecordTime};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const CLOCK: &str = "eventpipe.relative-ns";

pub fn clocks() -> Vec<Clock> {
    vec![Clock{id:CLOCK.into(),ticks_per_second:1_000_000_000,description:"integer projection from trace-header QPC origin/frequency; raw ticks retained; no implicit alignment to another source".into()}]
}

pub fn schemas() -> Vec<RecordSchema> {
    let nullable = |kind| FieldType::Nullable(Box::new(kind));
    vec![
        schema(
            "eventpipe.profile",
            "pinned TraceEvent extraction, source clock, loss and build identity",
            &[
                (
                    "format",
                    FieldType::String,
                    None,
                    "structured extraction protocol",
                ),
                (
                    "traceevent",
                    FieldType::String,
                    None,
                    "required extractor package version",
                ),
                (
                    "runtime",
                    FieldType::String,
                    None,
                    "actual CoreCLR runtime version",
                ),
                (
                    "frequency",
                    FieldType::Unsigned,
                    Some("ticks/second"),
                    "original trace-header QPC frequency",
                ),
                (
                    "origin_qpc",
                    FieldType::Signed,
                    Some("source ticks"),
                    "original trace-header clock origin",
                ),
                (
                    "events_lost",
                    FieldType::Unsigned,
                    Some("events"),
                    "loss count supplied by TraceEvent",
                ),
                (
                    "source_binding",
                    FieldType::Object,
                    None,
                    "executable/portable-pdb identity and original source binding",
                ),
                (
                    "warnings",
                    FieldType::Array,
                    None,
                    "explicit extraction and source-resolution limitations",
                ),
            ],
        ),
        schema(
            "eventpipe.frame",
            "code addresses and method/build/source evidence",
            &[
                (
                    "id",
                    FieldType::Unsigned,
                    None,
                    "source-local code address identity",
                ),
                (
                    "address",
                    FieldType::Unsigned,
                    Some("virtual address"),
                    "recorded instruction address",
                ),
                (
                    "function",
                    nullable(FieldType::String),
                    None,
                    "method name supplied by runtime rundown, if available",
                ),
                (
                    "module",
                    nullable(FieldType::String),
                    None,
                    "recorded module file path, if available",
                ),
                (
                    "module_pdb",
                    nullable(FieldType::String),
                    None,
                    "module PDB identity supplied by runtime rundown",
                ),
                (
                    "token",
                    nullable(FieldType::Unsigned),
                    None,
                    "managed method token, if available",
                ),
                (
                    "il_offset",
                    FieldType::Signed,
                    Some("IL bytes"),
                    "TraceEvent native-to-IL mapping; negative means unavailable",
                ),
                (
                    "source_file",
                    nullable(FieldType::String),
                    None,
                    "original source path only after matched portable-pdb and source-checksum checks",
                ),
                (
                    "source_line",
                    nullable(FieldType::Unsigned),
                    Some("source line"),
                    "matched sequence point or explicitly named method association",
                ),
                (
                    "source_mapping",
                    FieldType::String,
                    None,
                    "meaning or unavailability of the source association",
                ),
            ],
        ),
        schema(
            "eventpipe.stack",
            "source-local stack graph",
            &[
                (
                    "id",
                    FieldType::Unsigned,
                    None,
                    "source-local stack node identity",
                ),
                (
                    "frame",
                    FieldType::Unsigned,
                    None,
                    "source-local code address identity",
                ),
                (
                    "caller",
                    nullable(FieldType::Unsigned),
                    None,
                    "source-local caller stack or root",
                ),
            ],
        ),
        schema(
            "eventpipe.event",
            "events with original payload, timestamps, thread and stack identity",
            &[
                (
                    "id",
                    FieldType::Unsigned,
                    None,
                    "source-local event identity",
                ),
                (
                    "provider",
                    FieldType::String,
                    None,
                    "original provider name",
                ),
                ("name", FieldType::String, None, "original event name"),
                (
                    "provider_guid",
                    FieldType::String,
                    None,
                    "original provider GUID",
                ),
                ("event_id", FieldType::Unsigned, None, "original event id"),
                (
                    "version",
                    FieldType::Unsigned,
                    None,
                    "original event version",
                ),
                (
                    "activity",
                    FieldType::String,
                    None,
                    "original activity GUID; no operation ownership inferred",
                ),
                (
                    "related_activity",
                    FieldType::String,
                    None,
                    "original related-activity GUID",
                ),
                (
                    "process",
                    FieldType::Signed,
                    None,
                    "recorded process id within this process instance",
                ),
                ("thread", FieldType::Signed, None, "recorded thread id"),
                (
                    "time_qpc",
                    FieldType::Signed,
                    Some("source ticks"),
                    "original timestamp in the retained frequency/origin domain",
                ),
                (
                    "time_ns",
                    FieldType::Signed,
                    Some("nanoseconds"),
                    "checked integer projection relative to the trace header origin",
                ),
                (
                    "uncertainty_ns",
                    FieldType::Unsigned,
                    Some("nanoseconds"),
                    "projection resolution; does not bound alignment to another source",
                ),
                (
                    "stack",
                    nullable(FieldType::Unsigned),
                    None,
                    "source-local stack graph root",
                ),
                (
                    "payload",
                    FieldType::Object,
                    None,
                    "all supported decoded event payload fields",
                ),
            ],
        ),
    ]
}

pub fn extract(bytes: &[u8], subject: &str, limits: Limits) -> Result<Extraction> {
    let document = input(bytes, limits)?;
    if text(&document, "format", limits)? != "hunin-eventpipe-v1"
        || text(&document, "traceevent", limits)? != "4.0.0"
        || text(&document, "runtime", limits)? != "10.0.15"
    {
        return Err(Error(
            "unsupported EventPipe extraction or runtime version".into(),
        ));
    }
    let frequency = unsigned(&document, "frequency")?;
    if frequency == 0 {
        return Err(Error("EventPipe source frequency is zero".into()));
    }
    let origin = document
        .get("origin_qpc")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error("EventPipe source origin is not signed ticks".into()))?;
    let mut extraction = Extraction::new(
        limits,
        json!({"format":"hunin-eventpipe-v1","meaning":"EventPipe sampled thread time and runtime events; not scheduler-confirmed process cpu ownership"}),
    );
    extraction.schemas = schemas();
    extraction.clocks = clocks();
    let frames = array(&document, "frames")?;
    let stacks = array(&document, "stacks")?;
    let events = array(&document, "events")?;
    let mut frame_map = BTreeMap::new();
    let mut stack_map = BTreeMap::new();
    for value in frames {
        let frame = object(value)?;
        let id = unsigned(frame, "id")?;
        if frame_map.insert(id, value).is_some() {
            return Err(Error("duplicate EventPipe code address identity".into()));
        }
        let mut observed = record(
            format!("frame:{id}"),
            "eventpipe.frame",
            subject,
            value.clone(),
        );
        observed.function = nullable_text(frame, "function", limits)?.map(str::to_owned);
        let file = nullable_text(frame, "source_file", limits)?;
        let line = nullable_unsigned(frame, "source_line")?;
        if file.is_some() != line.is_some() {
            return Err(Error(
                "EventPipe source file/line association is incomplete".into(),
            ));
        }
        if let (Some(file), Some(line)) = (file, line) {
            if line == 0 {
                return Err(Error("EventPipe source line is zero".into()));
            }
            observed.location = Some(format!("{file}:{line}"));
        }
        extraction.push(observed)?;
    }
    for value in stacks {
        let stack = object(value)?;
        let id = unsigned(stack, "id")?;
        let frame = unsigned(stack, "frame")?;
        let caller = nullable_unsigned(stack, "caller")?;
        if !frame_map.contains_key(&frame) || stack_map.insert(id, (frame, caller)).is_some() {
            return Err(Error("invalid EventPipe stack/frame identity".into()));
        }
        extraction.push(record(
            format!("stack:{id}"),
            "eventpipe.stack",
            subject,
            value.clone(),
        ))?;
    }
    for (_, caller) in stack_map.values() {
        if caller.is_some_and(|caller| !stack_map.contains_key(&caller)) {
            return Err(Error("unresolved EventPipe caller stack".into()));
        }
    }
    let mut checked = BTreeSet::new();
    for &id in stack_map.keys() {
        let mut chain = BTreeSet::new();
        let mut next = Some(id);
        while let Some(id) = next {
            if checked.contains(&id) {
                break;
            }
            if chain.len() >= 1024 || !chain.insert(id) {
                return Err(Error("cyclic or excessive EventPipe stack depth".into()));
            }
            next = stack_map[&id].1;
        }
        checked.extend(chain);
    }
    let mut identities = BTreeSet::new();
    for value in events {
        let event = object(value)?;
        let id = unsigned(event, "id")?;
        if !identities.insert(id) {
            return Err(Error("duplicate EventPipe event identity".into()));
        }
        let ticks = event
            .get("time_qpc")
            .and_then(Value::as_i64)
            .ok_or_else(|| Error("EventPipe event source ticks absent".into()))?;
        let nanos = event
            .get("time_ns")
            .and_then(Value::as_i64)
            .ok_or_else(|| Error("EventPipe event projection absent".into()))?;
        let expected =
            (i128::from(ticks) - i128::from(origin)) * 1_000_000_000 / i128::from(frequency);
        if i128::from(nanos) != expected {
            return Err(Error(
                "EventPipe nanosecond projection differs from original clock".into(),
            ));
        }
        let uncertainty = unsigned(event, "uncertainty_ns")?;
        let minimum = 1_000_000_000_u64.div_ceil(frequency).max(1);
        if uncertainty < minimum {
            return Err(Error(
                "EventPipe projection understates source resolution".into(),
            ));
        }
        let stack = nullable_unsigned(event, "stack")?;
        if stack.is_some_and(|stack| !stack_map.contains_key(&stack)) {
            return Err(Error("unresolved EventPipe event stack".into()));
        }
        let mut observed = record(
            format!("event:{id}"),
            "eventpipe.event",
            subject,
            value.clone(),
        );
        observed.time = Some(RecordTime {
            clock: CLOCK.into(),
            start: nanos,
            end: None,
            uncertainty,
        });
        if let Some(stack) = stack {
            let frame = object(frame_map[&stack_map[&stack].0])?;
            observed.function = nullable_text(frame, "function", limits)?.map(str::to_owned);
            if let (Some(file), Some(line)) = (
                nullable_text(frame, "source_file", limits)?,
                nullable_unsigned(frame, "source_line")?,
            ) {
                observed.location = Some(format!("{file}:{line}"));
            }
        }
        extraction.push(observed)?;
    }
    let lost = unsigned(&document, "events_lost")?;
    if lost > 0 {
        extraction.capture_complete = false;
        extraction
            .warnings
            .push(format!("EventPipe reported {lost} lost events"));
    }
    for warning in array(&document, "warnings")? {
        let warning = warning
            .as_str()
            .filter(|warning| warning.len() <= limits.max_string_bytes)
            .ok_or_else(|| Error("invalid EventPipe warning".into()))?;
        extraction.warnings.push(warning.into());
    }
    let mut header = document;
    header.remove("frames");
    header.remove("stacks");
    header.remove("events");
    extraction.push(record(
        "profile".into(),
        "eventpipe.profile",
        subject,
        Value::Object(header),
    ))?;
    for observed in &extraction.records {
        let schema = extraction
            .schemas
            .iter()
            .find(|schema| schema.name == observed.schema)
            .expect("extractor schema");
        schema
            .validate_payload(&observed.payload)
            .map_err(|error| Error(error.to_string()))?;
    }
    Ok(extraction)
}

fn nullable_text<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
    limits: Limits,
) -> Result<Option<&'a str>> {
    match object.get(key) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.len() <= limits.max_string_bytes => Ok(Some(value)),
        _ => Err(Error(format!("invalid nullable EventPipe string {key}"))),
    }
}
fn nullable_unsigned(object: &serde_json::Map<String, Value>, key: &str) -> Result<Option<u64>> {
    match object.get(key) {
        Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| Error(format!("invalid EventPipe identity {key}"))),
        None => Err(Error(format!("missing EventPipe field {key}"))),
    }
}


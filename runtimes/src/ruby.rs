use crate::{
    Error, Extraction, Limits, Result, array, input, object, record, schema, text, unsigned,
};
use hunin_core::records::{Clock, FieldType, RecordTime};
use serde_json::{Value, json};

pub fn extract(bytes: &[u8], subject: &str, limits: Limits) -> Result<Extraction> {
    let mut root = input(bytes, limits)?;
    if text(&root, "runtime", limits)? != "ruby" {
        return Err(Error("expected CRuby trace".into()));
    }
    text(&root, "version", limits)?;
    let events = array(&root, "events")?;
    let mut extracted = Extraction::new(
        limits,
        json!({"format":"ruby.tracepoint","time_meaning":"monotonic callback time; includes observer overhead; not process cpu"}),
    );
    extracted.clocks = clocks();
    extracted.schemas = schemas();
    for (ordinal, event) in events.iter().enumerate() {
        let value = object(event)?;
        let kind = text(value, "event", limits)?;
        if ![
            "call",
            "return",
            "raise",
            "thread_begin",
            "thread_end",
            "fiber_switch",
            "b_call",
            "b_return",
            "c_call",
            "c_return",
        ]
        .contains(&kind)
        {
            extracted
                .warnings
                .push(format!("retained unknown TracePoint event kind {kind}"));
        }
        unsigned(value, "thread")?;
        unsigned(value, "fiber")?;
        let time = unsigned(value, "timestamp_ns")?;
        let mut event = record(
            format!("event:{ordinal}"),
            "ruby.event",
            subject,
            event.clone(),
        );
        event.time = i64::try_from(time).ok().map(|start| RecordTime {
            clock: "ruby-monotonic".into(),
            start,
            end: None,
            uncertainty: 0,
        });
        event.function = value
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let (Some(file), Some(line)) = (
            value.get("file").and_then(Value::as_str),
            value.get("line").and_then(Value::as_u64),
        ) {
            if file.len() > limits.max_string_bytes {
                return Err(Error("ruby source path exceeds budget".into()));
            }
            event.location = Some(format!("{file}:{line}"));
        }
        extracted.push(event)?;
    }
    let lost = unsigned(&root, "lost_events")?;
    if lost > 0 {
        extracted.capture_complete = false;
        extracted.warnings.push(format!(
            "TracePoint discarded {lost} callbacks at its buffer limit"
        ));
    }
    root.remove("events");
    extracted.push(record(
        "trace".into(),
        "ruby.trace",
        subject,
        Value::Object(root),
    ))?;
    Ok(extracted)
}

pub fn schemas() -> Vec<hunin_core::records::RecordSchema> {
    vec![
        schema(
            "ruby.trace",
            "trace runtime, selected events and coverage/loss",
            &[
                ("runtime", FieldType::String, None, "Ruby implementation"),
                (
                    "version",
                    FieldType::String,
                    None,
                    "actual interpreter version",
                ),
                (
                    "lost_events",
                    FieldType::Unsigned,
                    Some("events"),
                    "known discarded callbacks",
                ),
            ],
        ),
        schema(
            "ruby.event",
            "one automatically observed VM execution event",
            &[
                ("event", FieldType::String, None, "TracePoint event kind"),
                (
                    "method",
                    FieldType::Nullable(Box::new(FieldType::String)),
                    None,
                    "VM method identity when the event has one",
                ),
                (
                    "file",
                    FieldType::Nullable(Box::new(FieldType::String)),
                    None,
                    "source path supplied by the VM",
                ),
                (
                    "timestamp_ns",
                    FieldType::Unsigned,
                    Some("nanoseconds"),
                    "callback monotonic timestamp",
                ),
                (
                    "thread",
                    FieldType::Unsigned,
                    None,
                    "source-lifetime thread identity",
                ),
                (
                    "fiber",
                    FieldType::Unsigned,
                    None,
                    "source-lifetime fiber identity",
                ),
            ],
        ),
    ]
}

pub fn clocks() -> Vec<Clock> {
    vec![Clock {
        id: "ruby-monotonic".into(),
        ticks_per_second: 1_000_000_000,
        description: "producer Process::CLOCK_MONOTONIC; no controller-clock alignment implied"
            .into(),
    }]
}

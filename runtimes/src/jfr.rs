use crate::{
    Error, Extraction, Limits, Result, array, input, object, record, schema, text, unsigned,
};
use hunin_core::records::{Clock, FieldType, RecordSchema, RecordTime};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub fn extract(bytes: &[u8], subject: &str, limits: Limits) -> Result<Extraction> {
    let mut header = input(bytes, limits)?;
    if text(&header, "runtime", limits)? != "hotspot" {
        return Err(Error("expected HotSpot JFR extraction".into()));
    }
    text(&header, "version", limits)?;
    let samples = array(&header, "samples")?;
    let mut result = Extraction::new(
        limits,
        json!({"format":"jfr.execution-samples","meaning":"sampled execution stacks, not invocation counts or per-call cpu duration"}),
    );
    result.schemas = schemas();
    result.clocks = clocks();
    let mut mismatched = BTreeSet::new();
    for (index, sample) in samples.iter().enumerate() {
        let mut payload = object(sample)?.clone();
        let timestamp = payload
            .get("timestamp_ns")
            .and_then(Value::as_i64)
            .ok_or_else(|| Error("invalid JFR timestamp".into()))?;
        let frames = payload
            .remove("frames")
            .ok_or_else(|| Error("missing JFR stack".into()))?;
        let mut sample = record(
            format!("sample:{index}"),
            "jfr.sample",
            subject,
            Value::Object(payload),
        );
        sample.time = Some(RecordTime {
            clock: "jfr-instant".into(),
            start: timestamp,
            end: None,
            uncertainty: 0,
        });
        for (ordinal, frame) in frames
            .as_array()
            .ok_or_else(|| Error("JFR frames must be an array".into()))?
            .iter()
            .enumerate()
        {
            let mut payload = object(frame)?.clone();
            let class = text(&payload, "class", limits)?.to_string();
            let method = text(&payload, "method", limits)?.to_string();
            text(&payload, "descriptor", limits)?;
            let function = format!("{class}.{method}");
            if payload
                .get("expected_class_file_sha256")
                .is_some_and(Value::is_string)
                && payload
                    .get("class_file_sha256")
                    .is_some_and(Value::is_string)
                && payload["expected_class_file_sha256"] != payload["class_file_sha256"]
                && mismatched.insert(class.clone())
            {
                result.warnings.push(format!(
                    "class-file build mismatch for {class}; source location left unresolved"
                ));
            }
            payload.insert("sample".into(), json!(index));
            payload.insert("ordinal".into(), json!(ordinal));
            let mut frame = record(
                format!("frame:{index}:{ordinal}"),
                "jfr.frame",
                subject,
                Value::Object(payload),
            );
            frame.function = Some(function.clone());
            frame.time = sample.time.clone();
            if let (Some(source), Some(line)) = (
                frame.payload["source"].as_str(),
                frame.payload["line"].as_i64(),
            ) && line > 0
            {
                frame.location = Some(format!("{source}:{line}"));
            }
            if ordinal == 0 {
                sample.function = Some(function);
                sample.location = frame.location.clone();
            }
            result.push(frame)?;
        }
        result.push(sample)?;
    }
    let lost = unsigned(&header, "lost_samples")?;
    if lost > 0 {
        result.capture_complete = false;
        result.warnings.push(format!(
            "JFR extraction dropped {lost} samples at its budget"
        ));
    }
    header.remove("samples");
    result.push(record(
        "recording".into(),
        "jfr.recording",
        subject,
        Value::Object(header),
    ))?;
    Ok(result)
}

pub fn clocks() -> Vec<Clock> {
    vec![Clock{id:"jfr-instant".into(),ticks_per_second:1_000_000_000,
    description:"JDK RecordingFile decoded Instant; source resolution and helper clock alignment are not inferred".into()}]
}

pub fn schemas() -> Vec<RecordSchema> {
    vec![
        schema(
            "jfr.recording",
            "JFR settings, runtime and extraction coverage",
            &[
                ("runtime", FieldType::String, None, "JVM implementation"),
                (
                    "version",
                    FieldType::String,
                    None,
                    "actual JDK runtime version",
                ),
                (
                    "period_ns",
                    FieldType::Unsigned,
                    Some("nanoseconds"),
                    "requested execution-sampling period",
                ),
                (
                    "lost_samples",
                    FieldType::Unsigned,
                    Some("samples"),
                    "known extraction budget loss",
                ),
            ],
        ),
        schema(
            "jfr.sample",
            "one observed JFR execution sample",
            &[
                (
                    "timestamp_ns",
                    FieldType::Signed,
                    Some("nanoseconds"),
                    "JDK-decoded event timestamp",
                ),
                ("state", FieldType::String, None, "JFR sampled thread state"),
                (
                    "truncated",
                    FieldType::Boolean,
                    None,
                    "JFR reports stack truncation",
                ),
                (
                    "thread_java_id",
                    FieldType::Nullable(Box::new(FieldType::Signed)),
                    None,
                    "recorded Java thread id, null when absent",
                ),
                (
                    "thread_os_id",
                    FieldType::Nullable(Box::new(FieldType::Signed)),
                    None,
                    "recorded OS thread id, null when absent",
                ),
                (
                    "thread_name",
                    FieldType::Nullable(Box::new(FieldType::String)),
                    None,
                    "recorded Java thread name",
                ),
            ],
        ),
        schema(
            "jfr.frame",
            "one frame belonging to an execution sample",
            &[
                (
                    "sample",
                    FieldType::Unsigned,
                    None,
                    "source-local sample ordinal",
                ),
                (
                    "ordinal",
                    FieldType::Unsigned,
                    None,
                    "stack ordinal, leaf first",
                ),
                ("class", FieldType::String, None, "recorded declaring class"),
                (
                    "class_id",
                    FieldType::Unsigned,
                    None,
                    "recording-local class identity",
                ),
                (
                    "loader_id",
                    FieldType::Nullable(Box::new(FieldType::Unsigned)),
                    None,
                    "recording-local class-loader identity",
                ),
                ("method", FieldType::String, None, "recorded method name"),
                (
                    "descriptor",
                    FieldType::String,
                    None,
                    "JVM method descriptor",
                ),
                (
                    "line",
                    FieldType::Signed,
                    None,
                    "recorded line, negative means unavailable",
                ),
                (
                    "source",
                    FieldType::Nullable(Box::new(FieldType::String)),
                    None,
                    "matched prepared-class SourceFile or null",
                ),
                (
                    "class_file_sha256",
                    FieldType::Nullable(Box::new(FieldType::String)),
                    None,
                    "prepared class resource hash; does not prove loaded bytes after class redefinition",
                ),
                (
                    "expected_class_file_sha256",
                    FieldType::Nullable(Box::new(FieldType::String)),
                    None,
                    "prepared compiler manifest hash; mismatches prevent source binding",
                ),
            ],
        ),
    ]
}

use crate::{
    Error, Extraction, Limits, Result, array, input, object, record, schema, text, unsigned,
};
use hunin_core::records::{Clock, FieldType, RecordTime};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn extract(bytes: &[u8], subject: &str, limits: Limits) -> Result<Extraction> {
    let mut root = input(bytes, limits)?;
    let nodes = array(&root, "nodes")?;
    if nodes.is_empty() || nodes.len() > limits.max_records {
        return Err(Error("invalid profile node count".into()));
    }
    let start = root
        .get("startTime")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .ok_or_else(|| Error("invalid startTime".into()))?;
    let end = root
        .get("endTime")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .ok_or_else(|| Error("invalid endTime".into()))?;
    if start < 0.0 || end < start {
        return Err(Error("invalid profile clock interval".into()));
    }
    let mut frames = BTreeMap::new();
    let mut parents = BTreeMap::new();
    for node in nodes {
        let node = object(node)?;
        let id = unsigned(node, "id")?;
        let frame = object(
            node.get("callFrame")
                .ok_or_else(|| Error("missing callFrame".into()))?,
        )?;
        text(frame, "functionName", limits)?;
        text(frame, "scriptId", limits)?;
        text(frame, "url", limits)?;
        for field in ["lineNumber", "columnNumber"] {
            if frame.get(field).and_then(Value::as_i64).is_none() {
                return Err(Error(format!("invalid {field}")));
            }
        }
        if frames.insert(id, frame.clone()).is_some() {
            return Err(Error("duplicate profile node identity".into()));
        }
        if let Some(children) = node.get("children") {
            for child in children
                .as_array()
                .ok_or_else(|| Error("children must be an array".into()))?
            {
                let child = child
                    .as_u64()
                    .ok_or_else(|| Error("invalid child identity".into()))?;
                if parents.insert(child, id).is_some() {
                    return Err(Error("profile node has multiple parents".into()));
                }
            }
        }
    }
    let mut children_by_parent: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (&child, &parent) in &parents {
        if !frames.contains_key(&child) || !frames.contains_key(&parent) {
            return Err(Error("unresolved profile child".into()));
        }
        children_by_parent.entry(parent).or_default().push(child);
    }
    let roots: Vec<_> = frames
        .keys()
        .filter(|id| !parents.contains_key(id))
        .copied()
        .collect();
    if roots.len() != 1 {
        return Err(Error("profile must have one root".into()));
    }
    let mut pending = roots;
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            return Err(Error("cyclic profile ancestry".into()));
        }
        if let Some(children) = children_by_parent.get(&id) {
            pending.extend(children);
        }
    }
    if seen.len() != frames.len() {
        return Err(Error("disconnected or cyclic profile ancestry".into()));
    }
    let samples = root
        .get("samples")
        .map(|v| {
            v.as_array()
                .ok_or_else(|| Error("samples must be an array".into()))
        })
        .transpose()?;
    let deltas = root
        .get("timeDeltas")
        .map(|v| {
            v.as_array()
                .ok_or_else(|| Error("timeDeltas must be an array".into()))
        })
        .transpose()?;
    if samples.map(Vec::len) != deltas.map(Vec::len) {
        return Err(Error("sample and time-delta cardinalities differ".into()));
    }
    let mut extracted = Extraction::new(
        limits,
        json!({"format":"v8.cpu-profile","interpretation":"v8-profile-v1","samples_are_calls":false}),
    );
    extracted.clocks = clocks();
    extracted.schemas = schemas();
    if samples.is_none() {
        extracted.capture_complete = false;
        extracted
            .warnings
            .push("profile supplied no sample/time-delta stream".into());
    }
    for node in nodes {
        let mut payload = object(node)?.clone();
        let id = unsigned(&payload, "id")?;
        let unknown: Vec<_> = payload
            .keys()
            .filter(|k| {
                ![
                    "id",
                    "callFrame",
                    "hitCount",
                    "children",
                    "deoptReason",
                    "positionTicks",
                ]
                .contains(&k.as_str())
            })
            .cloned()
            .collect();
        if !unknown.is_empty() {
            extracted.warnings.push(format!(
                "node {id}: retained uninterpreted fields {unknown:?}"
            ));
        }
        let children = payload.remove("children");
        let ticks = payload.remove("positionTicks");
        let frame = &frames[&id];
        let mut result = record(
            format!("node:{id}"),
            "v8.node",
            subject,
            Value::Object(payload),
        );
        result.function = frame["functionName"]
            .as_str()
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        result.location = location(frame);
        extracted.push(result)?;
        if let Some(children) = children {
            for (ordinal, child) in children.as_array().unwrap().iter().enumerate() {
                let child = child.as_u64().unwrap();
                extracted.push(record(
                    format!("edge:{id}:{child}"),
                    "v8.edge",
                    subject,
                    json!({"parent":id,"child":child,"ordinal":ordinal}),
                ))?;
            }
        }
        if let Some(ticks) = ticks {
            for (ordinal, tick) in ticks
                .as_array()
                .ok_or_else(|| Error("positionTicks must be an array".into()))?
                .iter()
                .enumerate()
            {
                let mut tick = object(tick)?.clone();
                unsigned(&tick, "line")?;
                unsigned(&tick, "ticks")?;
                tick.insert("node".into(), json!(id));
                extracted.push(record(
                    format!("tick:{id}:{ordinal}"),
                    "v8.position-tick",
                    subject,
                    Value::Object(tick),
                ))?;
            }
        }
    }
    let mut timestamp = root["startTime"]
        .as_i64()
        .or_else(|| (start < i64::MAX as f64).then_some(start.floor() as i64));
    let uncertainty = u64::from(root["startTime"].as_i64().is_none());
    if let (Some(samples), Some(deltas)) = (samples, deltas) {
        let mut mapped_clock = true;
        for (ordinal, (node, delta)) in samples.iter().zip(deltas).enumerate() {
            let node = node
                .as_u64()
                .ok_or_else(|| Error("invalid sample node".into()))?;
            let frame = frames
                .get(&node)
                .ok_or_else(|| Error("unresolved sample node".into()))?;
            let delta = delta
                .as_i64()
                .ok_or_else(|| Error("invalid sample delta".into()))?;
            timestamp = timestamp.and_then(|value| value.checked_add(delta));
            if delta < 0 || timestamp.is_none() || timestamp.is_some_and(|value| value as f64 > end)
            {
                mapped_clock = false;
            }
            let mut sample = record(
                format!("sample:{ordinal}"),
                "v8.sample",
                subject,
                json!({"node":node,"delta_us":delta}),
            );
            if mapped_clock {
                sample.time = Some(RecordTime {
                    clock: "v8-profile".into(),
                    start: timestamp.unwrap(),
                    end: None,
                    uncertainty,
                });
            }
            sample.function = frame["functionName"]
                .as_str()
                .filter(|v| !v.is_empty())
                .map(str::to_string);
            sample.location = location(frame);
            extracted.push(sample)?;
        }
        if !mapped_clock {
            extracted
                .warnings
                .push("invalid source deltas: subsequent timestamps remain unprojected".into());
        }
    }
    root.remove("nodes");
    root.remove("samples");
    root.remove("timeDeltas");
    extracted.push(record(
        "profile".into(),
        "v8.profile",
        subject,
        Value::Object(root),
    ))?;
    Ok(extracted)
}

fn location(frame: &serde_json::Map<String, Value>) -> Option<String> {
    let url = frame.get("url")?.as_str()?;
    let line = frame.get("lineNumber")?.as_i64()?;
    let column = frame.get("columnNumber")?.as_i64()?;
    if url.is_empty() || line < 0 || column < 0 {
        None
    } else {
        Some(format!("{url}:{}:{}", line + 1, column + 1))
    }
}

pub fn schemas() -> Vec<hunin_core::records::RecordSchema> {
    vec![
        schema(
            "v8.profile",
            "profile header and source-native extension fields",
            &[
                (
                    "startTime",
                    FieldType::Float,
                    Some("microseconds"),
                    "profile source clock start",
                ),
                (
                    "endTime",
                    FieldType::Float,
                    Some("microseconds"),
                    "profile source clock end",
                ),
            ],
        ),
        schema(
            "v8.node",
            "profile call-tree node; not a call invocation",
            &[
                (
                    "id",
                    FieldType::Unsigned,
                    None,
                    "source-local node identity",
                ),
                (
                    "callFrame",
                    FieldType::Object,
                    None,
                    "original function, script, and generated position",
                ),
            ],
        ),
        schema(
            "v8.edge",
            "call-tree structure; no inferred invocation count",
            &[
                ("parent", FieldType::Unsigned, None, "parent node"),
                ("child", FieldType::Unsigned, None, "child node"),
            ],
        ),
        schema(
            "v8.sample",
            "one retained sampling observation",
            &[
                ("node", FieldType::Unsigned, None, "sampled leaf node"),
                (
                    "delta_us",
                    FieldType::Signed,
                    Some("microseconds"),
                    "source delta from preceding sample or profile start",
                ),
            ],
        ),
        schema(
            "v8.position-tick",
            "source-native line sampling weights",
            &[
                ("node", FieldType::Unsigned, None, "profile node"),
                (
                    "line",
                    FieldType::Unsigned,
                    None,
                    "source-native line convention",
                ),
                (
                    "ticks",
                    FieldType::Unsigned,
                    Some("ticks"),
                    "sampling ticks, not invocation count",
                ),
            ],
        ),
    ]
}

pub fn clocks() -> Vec<Clock> {
    vec![Clock {
        id: "v8-profile".into(),
        ticks_per_second: 1_000_000,
        description: "producer profile clock; origin is producer-specific, with no implicit controller-clock alignment"
            .into(),
    }]
}

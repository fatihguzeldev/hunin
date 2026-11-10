use crate::{Error, Extraction, Limits, Result, record, schema};
use hunin_core::records::{FieldType, RecordSchema};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

pub fn schemas() -> Vec<RecordSchema> {
    vec![
        schema(
            "xdebug.profile",
            "xdebug cachegrind header and finalization evidence",
            &[
                (
                    "creator",
                    FieldType::String,
                    None,
                    "producer identity supplied by the profile",
                ),
                (
                    "events",
                    FieldType::Array,
                    None,
                    "ordered original event definitions",
                ),
                (
                    "headers",
                    FieldType::Array,
                    None,
                    "original header keys and values",
                ),
                (
                    "summary",
                    FieldType::Array,
                    None,
                    "original summary costs; not a sum of self costs",
                ),
                (
                    "complete",
                    FieldType::Boolean,
                    None,
                    "summary and all pending call associations were present",
                ),
            ],
        ),
        schema(
            "xdebug.file",
            "source-owned compressed file identities",
            &[
                (
                    "id",
                    FieldType::Unsigned,
                    None,
                    "source-local file reference",
                ),
                (
                    "name",
                    FieldType::String,
                    None,
                    "original file name or php:internal",
                ),
            ],
        ),
        schema(
            "xdebug.function",
            "source-owned compressed function identities",
            &[
                (
                    "id",
                    FieldType::Unsigned,
                    None,
                    "source-local function reference",
                ),
                ("name", FieldType::String, None, "original function name"),
            ],
        ),
        schema(
            "xdebug.cost",
            "per-entry exclusive profile costs at the supplied declaration line",
            &[
                ("file", FieldType::String, None, "supplied source file"),
                (
                    "function",
                    FieldType::String,
                    None,
                    "supplied function name",
                ),
                (
                    "line",
                    FieldType::Unsigned,
                    Some("source line"),
                    "one-based declaration position supplied by xdebug",
                ),
                (
                    "time_10ns",
                    FieldType::Unsigned,
                    Some("10ns"),
                    "exclusive profiler elapsed cost, not process cpu time",
                ),
                (
                    "memory_bytes",
                    FieldType::Unsigned,
                    Some("bytes"),
                    "nonnegative Zend allocation cost supplied by xdebug, not process rss",
                ),
            ],
        ),
        schema(
            "xdebug.call",
            "call counts and inclusive costs supplied by xdebug",
            &[
                (
                    "caller_file",
                    FieldType::String,
                    None,
                    "supplied caller file",
                ),
                (
                    "caller_function",
                    FieldType::String,
                    None,
                    "supplied caller function",
                ),
                (
                    "caller_cost",
                    FieldType::String,
                    None,
                    "source-local exclusive record belonging to this call list",
                ),
                (
                    "callee_file",
                    FieldType::String,
                    None,
                    "supplied callee file",
                ),
                (
                    "callee_function",
                    FieldType::String,
                    None,
                    "supplied callee function",
                ),
                (
                    "line",
                    FieldType::Unsigned,
                    Some("source line"),
                    "supplied caller-side call position",
                ),
                (
                    "calls",
                    FieldType::Unsigned,
                    Some("calls"),
                    "call count supplied by the association",
                ),
                (
                    "target_positions",
                    FieldType::Array,
                    None,
                    "all original target-position values; zero does not identify a source statement",
                ),
                (
                    "time_10ns",
                    FieldType::Unsigned,
                    Some("10ns"),
                    "inclusive profiler elapsed cost of the association",
                ),
                (
                    "memory_bytes",
                    FieldType::Unsigned,
                    Some("bytes"),
                    "inclusive nonnegative Zend allocation cost supplied by xdebug",
                ),
            ],
        ),
    ]
}

pub fn extract(bytes: &[u8], subject: &str, limits: Limits) -> Result<Extraction> {
    if bytes.len() > limits.max_input_bytes {
        return Err(Error("xdebug input exceeds byte budget".into()));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| {
        Error(
            "xdebug profile is not utf8; compressed input must be disabled or decoded explicitly"
                .into(),
        )
    })?;
    let mut extraction = Extraction::new(
        limits,
        json!({"format":"xdebug-cachegrind-v1","cost_clock":"aggregate elapsed costs; no per-entry timestamps","memory_scope":"Zend allocation cost, not resident memory"}),
    );
    extraction.schemas = schemas();
    let mut files = BTreeMap::new();
    let mut functions = BTreeMap::new();
    let mut headers = Vec::new();
    let mut events = Vec::<String>::new();
    let mut summary = Vec::<u64>::new();
    let mut creator = None;
    let mut version = None;
    let mut positions = None;
    let mut file = None;
    let mut function = None;
    let mut callee_file = None;
    let mut callee_function = None;
    let mut pending = None;
    let mut caller_cost = None;
    let mut saw_summary = false;
    for (index, line) in text.lines().enumerate() {
        if line.len() > limits.max_string_bytes {
            return Err(Error(format!(
                "xdebug line {} exceeds string budget",
                index + 1
            )));
        }
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(": ")
            && !key.contains('=')
        {
            if pending.is_some() {
                return Err(Error("xdebug header interrupts a pending call cost".into()));
            }
            match key {
                "version" => {
                    if version.replace(value.to_owned()).is_some() {
                        return Err(Error(
                            "multiple xdebug profile parts are unsupported".into(),
                        ));
                    }
                }
                "creator" => creator = Some(value.to_owned()),
                "positions" => positions = Some(value.to_owned()),
                "events" => {
                    if !events.is_empty() {
                        return Err(Error("multiple event definitions are unsupported".into()));
                    }
                    events = value.split_whitespace().map(str::to_owned).collect();
                }
                "summary" => {
                    if saw_summary {
                        return Err(Error("duplicate xdebug summary".into()));
                    }
                    summary = parse_numbers(value)?;
                    saw_summary = true;
                }
                _ => {}
            }
            headers.push(json!({"name":key,"value":value}));
            continue;
        }
        if saw_summary {
            return Err(Error("xdebug data follows the final summary".into()));
        }
        if let Some(value) = line.strip_prefix("fl=") {
            if pending.is_some() {
                return Err(Error("xdebug file change interrupts a pending call".into()));
            }
            file = Some(resolve_name(
                value,
                &mut files,
                "xdebug.file",
                subject,
                &mut extraction,
            )?);
            function = None;
            callee_file = None;
            callee_function = None;
            caller_cost = None;
            continue;
        }
        if let Some(value) = line.strip_prefix("fn=") {
            if pending.is_some() {
                return Err(Error(
                    "xdebug function change interrupts a pending call".into(),
                ));
            }
            function = Some(resolve_name(
                value,
                &mut functions,
                "xdebug.function",
                subject,
                &mut extraction,
            )?);
            callee_file = None;
            callee_function = None;
            caller_cost = None;
            continue;
        }
        if let Some(value) = line
            .strip_prefix("cfl=")
            .or_else(|| line.strip_prefix("cfi="))
        {
            callee_file = Some(resolve_name(
                value,
                &mut files,
                "xdebug.file",
                subject,
                &mut extraction,
            )?);
            continue;
        }
        if let Some(value) = line.strip_prefix("cfn=") {
            callee_function = Some(resolve_name(
                value,
                &mut functions,
                "xdebug.function",
                subject,
                &mut extraction,
            )?);
            continue;
        }
        if let Some(value) = line.strip_prefix("calls=") {
            if pending.is_some() {
                return Err(Error("xdebug call association has no cost".into()));
            }
            let mut values = parse_numbers(value)?.into_iter();
            let count = values
                .next()
                .ok_or_else(|| Error("xdebug call count is absent".into()))?;
            let target_positions = values.collect::<Vec<_>>();
            if target_positions.is_empty() {
                return Err(Error("xdebug call target positions are absent".into()));
            }
            pending = Some((count, target_positions));
            continue;
        }
        validate_cost_headers(
            version.as_deref(),
            creator.as_deref(),
            positions.as_deref(),
            &events,
        )?;
        let values = parse_numbers(line)?;
        if values.len() != 3 {
            return Err(Error(format!(
                "xdebug cost at line {} requires one position and two exact counters",
                index + 1
            )));
        }
        let (file, function) = (
            file.as_ref()
                .ok_or_else(|| Error("xdebug cost has no file".into()))?,
            function
                .as_ref()
                .ok_or_else(|| Error("xdebug cost has no function".into()))?,
        );
        let time = values[1 + events
            .iter()
            .position(|event| event == "Time_(10ns)")
            .expect("validated events")];
        let memory = values[1 + events
            .iter()
            .position(|event| event == "Memory_(bytes)")
            .expect("validated events")];
        let (kind, payload, observed_function) = if let Some((calls, target_positions)) =
            pending.take()
        {
            let callee = callee_function
                .as_ref()
                .ok_or_else(|| Error("xdebug call has no callee function".into()))?;
            (
                "xdebug.call",
                json!({"caller_file":file,"caller_function":function,"caller_cost":caller_cost.as_ref().ok_or_else(||Error("xdebug call has no preceding self cost".into()))?,"callee_file":callee_file.as_ref().unwrap_or(file),"callee_function":callee,"line":values[0],"calls":calls,"target_positions":target_positions,"time_10ns":time,"memory_bytes":memory}),
                callee,
            )
        } else {
            caller_cost = Some(format!("cost:{}", index + 1));
            (
                "xdebug.cost",
                json!({"file":file,"function":function,"line":values[0],"time_10ns":time,"memory_bytes":memory}),
                function,
            )
        };
        let mut observed = record(
            format!(
                "{}:{}",
                if kind == "xdebug.call" {
                    "call"
                } else {
                    "cost"
                },
                index + 1
            ),
            kind,
            subject,
            payload,
        );
        observed.function = Some(observed_function.clone());
        observed.location = Some(format!("{file}:{}", values[0]));
        extraction.push(observed)?;
    }
    validate_cost_headers(
        version.as_deref(),
        creator.as_deref(),
        positions.as_deref(),
        &events,
    )?;
    let complete = saw_summary && pending.is_none();
    extraction.capture_complete = complete;
    if !complete {
        extraction.warnings.push("xdebug profile lacks a final summary or ends inside a call association; retained prefix is incomplete".into());
    }
    if saw_summary && summary.len() != events.len() {
        return Err(Error(
            "xdebug summary counter count does not match events".into(),
        ));
    }
    let profile = record(
        "profile".into(),
        "xdebug.profile",
        subject,
        json!({"creator":creator.expect("validated creator"),"events":events,"headers":headers,"summary":summary,"complete":complete}),
    );
    extraction.push(profile)?;
    Ok(extraction)
}

fn validate_cost_headers(
    version: Option<&str>,
    creator: Option<&str>,
    positions: Option<&str>,
    events: &[String],
) -> Result<()> {
    if version != Some("1")
        || !creator.is_some_and(|value| value.starts_with("xdebug 3.5.3 (PHP "))
        || positions != Some("line")
    {
        return Err(Error(
            "supported input requires xdebug 3.5.3, cachegrind version 1 and line positions".into(),
        ));
    }
    if events.len() != 2
        || events.iter().map(String::as_str).collect::<BTreeSet<_>>()
            != BTreeSet::from(["Time_(10ns)", "Memory_(bytes)"])
    {
        return Err(Error(
            "unsupported xdebug event units; expected Time_(10ns) and Memory_(bytes)".into(),
        ));
    }
    Ok(())
}

fn parse_numbers(value: &str) -> Result<Vec<u64>> {
    value
        .split_whitespace()
        .map(|value| {
            value
                .parse()
                .map_err(|_| Error(format!("unsupported xdebug counter or position: {value}")))
        })
        .collect()
}

fn resolve_name(
    value: &str,
    names: &mut BTreeMap<u64, String>,
    kind: &str,
    subject: &str,
    extraction: &mut Extraction,
) -> Result<String> {
    if let Some(value) = value.strip_prefix('(') {
        let (id, tail) = value
            .split_once(')')
            .ok_or_else(|| Error("malformed compressed xdebug name".into()))?;
        let id: u64 = id
            .parse()
            .map_err(|_| Error("invalid xdebug name identity".into()))?;
        if tail.is_empty() {
            return names
                .get(&id)
                .cloned()
                .ok_or_else(|| Error(format!("unresolved xdebug name identity {id}")));
        }
        let name = tail
            .strip_prefix(' ')
            .filter(|name| !name.is_empty())
            .ok_or_else(|| Error("empty xdebug name".into()))?;
        if let Some(old) = names.get(&id) {
            if old != name {
                return Err(Error(format!("conflicting xdebug name identity {id}")));
            }
            return Ok(name.into());
        }
        names.insert(id, name.into());
        extraction.push(record(
            format!("{kind}:{id}"),
            kind,
            subject,
            json!({"id":id,"name":name}),
        ))?;
        Ok(name.into())
    } else if value.is_empty() {
        Err(Error("empty xdebug name".into()))
    } else {
        Ok(value.into())
    }
}


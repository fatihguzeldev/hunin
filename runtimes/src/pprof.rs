//! pprof's gzip protobuf data, including source units and unresolved extensions.
use crate::{Error, Extraction, Limits, Result, record, schema};
use flate2::bufread::GzDecoder;
use hunin_core::records::{FieldType, RecordSchema};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

#[derive(Clone, Debug)]
enum Wire {
    Int(u64),
    Bytes(Vec<u8>),
    Fixed(Vec<u8>),
}
type Message = BTreeMap<u32, Vec<Wire>>;

fn varint(bytes: &[u8], cursor: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes
            .get(*cursor)
            .ok_or_else(|| Error("truncated protobuf varint".into()))?;
        *cursor += 1;
        if shift == 63 && byte > 1 {
            return Err(Error("protobuf varint overflow".into()));
        }
        value |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err(Error("protobuf varint overflow".into()))
}
fn message(bytes: &[u8]) -> Result<Message> {
    let mut fields: Message = BTreeMap::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let key = varint(bytes, &mut cursor)?;
        let tag = u32::try_from(key >> 3).map_err(|_| Error("protobuf tag overflow".into()))?;
        if tag == 0 || tag > 536870911 {
            return Err(Error("invalid protobuf tag".into()));
        }
        let wire = match key & 7 {
            0 => Wire::Int(varint(bytes, &mut cursor)?),
            1 | 2 | 5 => {
                let length = match key & 7 {
                    1 => 8,
                    5 => 4,
                    _ => usize::try_from(varint(bytes, &mut cursor)?)
                        .map_err(|_| Error("protobuf length overflow".into()))?,
                };
                let end = cursor
                    .checked_add(length)
                    .filter(|end| *end <= bytes.len())
                    .ok_or_else(|| Error("truncated protobuf field".into()))?;
                let payload = bytes[cursor..end].to_vec();
                cursor = end;
                if key & 7 == 2 {
                    Wire::Bytes(payload)
                } else {
                    Wire::Fixed(payload)
                }
            }
            _ => return Err(Error("unsupported protobuf wire type".into())),
        };
        fields.entry(tag).or_default().push(wire);
    }
    Ok(fields)
}
fn number(fields: &Message, tag: u32) -> Result<u64> {
    match fields.get(&tag).and_then(|v| v.last()) {
        None => Ok(0),
        Some(Wire::Int(n)) => Ok(*n),
        _ => Err(Error(format!("pprof field {tag} has wrong wire type"))),
    }
}
fn packed(fields: &Message, tag: u32) -> Result<Vec<u64>> {
    let mut output = vec![];
    for value in fields.get(&tag).into_iter().flatten() {
        match value {
            Wire::Int(n) => output.push(*n),
            Wire::Bytes(bytes) => {
                let mut pos = 0;
                while pos < bytes.len() {
                    output.push(varint(bytes, &mut pos)?);
                }
            }
            _ => return Err(Error("wrong repeated integer wire type".into())),
        }
    }
    Ok(output)
}
fn children(fields: &Message, tag: u32) -> Result<Vec<Message>> {
    fields
        .get(&tag)
        .into_iter()
        .flatten()
        .map(|wire| match wire {
            Wire::Bytes(bytes) => message(bytes),
            _ => Err(Error("wrong nested message wire type".into())),
        })
        .collect()
}
fn string(strings: &[String], index: u64) -> Result<&str> {
    strings
        .get(usize::try_from(index).map_err(|_| Error("pprof string index overflow".into()))?)
        .map(String::as_str)
        .ok_or_else(|| Error("pprof string index out of range".into()))
}
fn extensions(fields: &Message, known: &[u32]) -> Value {
    let mut unknown = BTreeMap::new();
    for (tag, values) in fields {
        if !known.contains(tag) {
            unknown.insert(
                tag.to_string(),
                values
                    .iter()
                    .map(|w| match w {
                        Wire::Int(n) => json!({"wire":0,"unsigned":n}),
                        Wire::Bytes(b) => json!({"wire":2,"bytes":b}),
                        Wire::Fixed(b) => json!({"wire":if b.len()==8 {1}else{5},"bytes":b}),
                    })
                    .collect::<Vec<_>>(),
            );
        }
    }
    json!(unknown)
}
fn value_type(fields: &Message, strings: &[String]) -> Result<Value> {
    Ok(
        json!({"type":string(strings,number(fields,1)?)?,"unit":string(strings,number(fields,2)?)?,"extensions":extensions(fields,&[1,2])}),
    )
}
pub fn schemas() -> Vec<RecordSchema> {
    vec![
        schema(
            "pprof.profile",
            "pprof aggregate profile and its declared sample value units",
            &[
                (
                    "sample_types",
                    FieldType::Array,
                    None,
                    "ordered value types and units",
                ),
                (
                    "duration_nanos",
                    FieldType::Signed,
                    Some("ns"),
                    "source duration; not per-sample event time",
                ),
                (
                    "period",
                    FieldType::Signed,
                    None,
                    "period in declared period_type units",
                ),
            ],
        ),
        schema(
            "pprof.function",
            "pprof source function dictionary",
            &[
                (
                    "id",
                    FieldType::Unsigned,
                    None,
                    "source-local nonzero function id",
                ),
                ("name", FieldType::String, None, "source symbol name"),
                (
                    "filename",
                    FieldType::String,
                    None,
                    "source file from runtime symbolization",
                ),
            ],
        ),
        schema(
            "pprof.mapping",
            "pprof image mapping and build identity",
            &[
                ("id", FieldType::Unsigned, None, "source mapping id"),
                (
                    "build_id",
                    FieldType::String,
                    None,
                    "producer build identity; empty is unknown",
                ),
            ],
        ),
        schema(
            "pprof.location",
            "pprof instruction address with ordered inline frames",
            &[
                ("id", FieldType::Unsigned, None, "source location id"),
                (
                    "address",
                    FieldType::Unsigned,
                    None,
                    "source virtual address",
                ),
                (
                    "lines",
                    FieldType::Array,
                    None,
                    "inline callee first; preserves source line/column",
                ),
            ],
        ),
        schema(
            "pprof.sample",
            "pprof aggregate leaf-first stack with typed values and labels",
            &[
                (
                    "location_ids",
                    FieldType::Array,
                    None,
                    "ordered leaf-first location ids",
                ),
                (
                    "values",
                    FieldType::Array,
                    None,
                    "signed values in profile sample_types units",
                ),
                (
                    "labels",
                    FieldType::Array,
                    None,
                    "all string/numeric label values and units",
                ),
            ],
        ),
    ]
}
pub fn extract(bytes: &[u8], subject: &str, limits: Limits) -> Result<Extraction> {
    if bytes.len() > limits.max_input_bytes {
        return Err(Error("compressed pprof byte budget exceeded".into()));
    }
    let mut raw = vec![];
    let mut decoder = GzDecoder::new(bytes);
    (&mut decoder)
        .take(limits.max_input_bytes as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|e| Error(e.to_string()))?;
    if raw.len() > limits.max_input_bytes {
        return Err(Error("decompressed pprof byte budget exceeded".into()));
    }
    if !decoder.into_inner().is_empty() {
        return Err(Error("trailing bytes after pprof gzip member".into()));
    }
    let root = message(&raw)?;
    let dictionary_records = [2, 3, 4, 5]
        .into_iter()
        .map(|tag| root.get(&tag).map_or(0, Vec::len))
        .sum::<usize>();
    if dictionary_records >= limits.max_records {
        return Err(Error("pprof record budget exceeded".into()));
    }
    let mut strings = vec![];
    for wire in root.get(&6).into_iter().flatten() {
        match wire {
            Wire::Bytes(bytes) => {
                if bytes.len() > limits.max_string_bytes {
                    return Err(Error("pprof string budget exceeded".into()));
                }
                strings.push(String::from_utf8(bytes.clone()).map_err(|e| Error(e.to_string()))?);
            }
            _ => return Err(Error("invalid pprof string table".into())),
        }
    }
    if strings.first().map(String::as_str) != Some("") {
        return Err(Error("pprof string_table[0] must be empty".into()));
    }
    let sample_types = children(&root, 1)?
        .iter()
        .map(|v| value_type(v, &strings))
        .collect::<Result<Vec<_>>>()?;
    if sample_types.is_empty() {
        return Err(Error("pprof has no value types".into()));
    }
    let period_types = children(&root, 11)?;
    let period_type = period_types
        .last()
        .map(|v| value_type(v, &strings))
        .transpose()?;
    let mut out = Extraction::new(
        limits,
        json!({"format":"pprof-protobuf-v1","units":"declared per value; no implicit CPU/wall conversion","time":"profile epoch/time and duration are aggregate metadata; samples have no individual timestamps","extensions":"unknown numbered fields retained with wire type and exact payload bytes"}),
    );
    out.schemas = schemas();
    out.push(record("profile".into(),"pprof.profile",subject,json!({"string_table":strings,"sample_types":sample_types,"period_type":period_type,"period":number(&root,12)? as i64,"time_nanos":number(&root,9)? as i64,"duration_nanos":number(&root,10)? as i64,"drop_frames":string(&strings,number(&root,7)?)?,"keep_frames":string(&strings,number(&root,8)?)?,"comments":packed(&root,13)?.into_iter().map(|n|string(&strings,n).map(str::to_string)).collect::<Result<Vec<_>>>()?,"default_sample_type":string(&strings,number(&root,14)?)?,"doc_url":string(&strings,number(&root,15)?)?,"extensions":extensions(&root,&[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15])})))?;
    let mut mappings = BTreeSet::new();
    let mut loss_locations = BTreeSet::new();
    let mut loss_samples = 0usize;
    let mut functions = BTreeMap::new();
    let mut locations = BTreeMap::new();
    for m in children(&root, 3)? {
        let id = number(&m, 1)?;
        if id == 0 || !mappings.insert(id) {
            return Err(Error("duplicate/zero pprof mapping id".into()));
        }
        let start = number(&m, 2)?;
        let end = number(&m, 3)?;
        if end < start {
            return Err(Error("reversed pprof mapping".into()));
        }
        out.push(record(format!("mapping:{id}"),"pprof.mapping",subject,json!({"id":id,"memory_start":start,"memory_limit":end,"file_offset":number(&m,4)?,"filename":string(&strings,number(&m,5)?)?,"build_id":string(&strings,number(&m,6)?)?,"has_functions":number(&m,7)?!=0,"has_filenames":number(&m,8)?!=0,"has_line_numbers":number(&m,9)?!=0,"has_inline_frames":number(&m,10)?!=0,"extensions":extensions(&m,&[1,2,3,4,5,6,7,8,9,10])})))?;
    }
    for f in children(&root, 5)? {
        let id = number(&f, 1)?;
        if id == 0 || functions.contains_key(&id) {
            return Err(Error("duplicate/zero pprof function id".into()));
        }
        let name = string(&strings, number(&f, 2)?)?;
        let file = string(&strings, number(&f, 4)?)?;
        functions.insert(id, (name.to_string(), file.to_string()));
        let mut row = record(
            format!("function:{id}"),
            "pprof.function",
            subject,
            json!({"id":id,"name":name,"system_name":string(&strings,number(&f,3)?)?,"filename":file,"start_line":number(&f,5)? as i64,"extensions":extensions(&f,&[1,2,3,4,5])}),
        );
        row.function = (!name.is_empty()).then(|| name.into());
        out.push(row)?;
    }
    for l in children(&root, 4)? {
        let id = number(&l, 1)?;
        let mapping = number(&l, 2)?;
        if id == 0 || locations.contains_key(&id) || (mapping != 0 && !mappings.contains(&mapping))
        {
            return Err(Error("invalid pprof location identity/mapping".into()));
        }
        let mut lines = vec![];
        let mut projection = None;
        for line in children(&l, 4)? {
            let function_id = number(&line, 1)?;
            let Some((name, file)) = functions.get(&function_id) else {
                return Err(Error(
                    "pprof inline frame references missing function".into(),
                ));
            };
            if [
                "runtime/pprof.lostProfileEvent",
                "runtime._LostExternalCode",
                "runtime._LostSIGPROFDuringAtomic64",
            ]
            .contains(&name.as_str())
            {
                loss_locations.insert(id);
            }
            let line_number = number(&line, 2)? as i64;
            let column = number(&line, 3)? as i64;
            if projection.is_none() {
                projection = Some((
                    name.clone(),
                    (!file.is_empty() && line_number > 0)
                        .then(|| format!("{file}:{line_number}:{column}")),
                ));
            }
            lines.push(json!({"function_id":function_id,"line":line_number,"column":column,"extensions":extensions(&line,&[1,2,3])}));
        }
        locations.insert(id, projection.clone());
        let mut row = record(
            format!("location:{id}"),
            "pprof.location",
            subject,
            json!({"id":id,"mapping_id":mapping,"address":number(&l,3)?,"lines":lines,"is_folded":number(&l,5)?!=0,"extensions":extensions(&l,&[1,2,3,4,5])}),
        );
        if let Some((name, location)) = projection {
            row.function = Some(name);
            row.location = location;
        }
        out.push(row)?;
    }
    for (index, sample) in children(&root, 2)?.into_iter().enumerate() {
        let ids = packed(&sample, 1)?;
        if ids.iter().any(|id| loss_locations.contains(id)) {
            out.capture_complete = false;
            loss_samples += 1;
        }
        let values = packed(&sample, 2)?
            .into_iter()
            .map(|n| n as i64)
            .collect::<Vec<_>>();
        if values.len() != sample_types.len() || ids.iter().any(|id| !locations.contains_key(id)) {
            return Err(Error(
                "pprof sample value cardinality or location reference invalid".into(),
            ));
        }
        let labels=children(&sample,3)?.into_iter().map(|l|Ok(json!({"key":string(&strings,number(&l,1)?)?,"string":string(&strings,number(&l,2)?)?,"number":number(&l,3)? as i64,"number_unit":string(&strings,number(&l,4)?)?,"extensions":extensions(&l,&[1,2,3,4])}))).collect::<Result<Vec<Value>>>()?;
        let mut row = record(
            format!("sample:{index}"),
            "pprof.sample",
            subject,
            json!({"location_ids":ids,"values":values,"labels":labels,"extensions":extensions(&sample,&[1,2,3])}),
        );
        if let Some(Some((name, location))) = ids.first().and_then(|id| locations.get(id)) {
            row.function = Some(name.clone());
            row.location = location.clone();
        }
        out.push(row)?;
    }
    if loss_samples > 0 {
        out.warnings.push(format!("{loss_samples} pprof rows contain Go runtime loss markers; original values and stacks retained"));
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionCost {
    pub id: u64,
    pub name: String,
    pub filename: String,
    pub self_value: i64,
    pub inclusive_value: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Costs {
    pub value_type: String,
    pub unit: String,
    pub functions: Vec<FunctionCost>,
    pub unresolved_self_value: i64,
}

/// queries one retained source's records without reopening its protobuf container.
/// recursion contributes once per function to a sample's inclusive value.
pub fn costs(
    records: &[hunin_core::records::Record],
    value_type: &str,
    unit: &str,
) -> Result<Costs> {
    let headers = records
        .iter()
        .filter(|r| r.schema == "pprof.profile")
        .collect::<Vec<_>>();
    if headers.len() != 1 {
        return Err(Error("pprof query requires one source profile".into()));
    }
    let types = headers[0].payload["sample_types"]
        .as_array()
        .ok_or_else(|| Error("missing pprof value type table".into()))?;
    let positions = types
        .iter()
        .enumerate()
        .filter(|(_, v)| v["type"].as_str() == Some(value_type) && v["unit"].as_str() == Some(unit))
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    if positions.len() != 1 {
        return Err(Error(
            "requested pprof value type/unit is missing or ambiguous".into(),
        ));
    }
    let index = positions[0];
    let mut functions = BTreeMap::new();
    let mut locations = BTreeMap::new();
    for row in records.iter().filter(|r| r.schema == "pprof.function") {
        let id = row.payload["id"]
            .as_u64()
            .ok_or_else(|| Error("missing pprof function id".into()))?;
        let cost = FunctionCost {
            id,
            name: row.payload["name"]
                .as_str()
                .ok_or_else(|| Error("missing pprof function name".into()))?
                .into(),
            filename: row.payload["filename"]
                .as_str()
                .ok_or_else(|| Error("missing pprof function filename".into()))?
                .into(),
            self_value: 0,
            inclusive_value: 0,
        };
        if functions.insert(id, cost).is_some() {
            return Err(Error("duplicate pprof function".into()));
        }
    }
    for row in records.iter().filter(|r| r.schema == "pprof.location") {
        let id = row.payload["id"]
            .as_u64()
            .ok_or_else(|| Error("missing pprof location id".into()))?;
        let lines = row.payload["lines"]
            .as_array()
            .ok_or_else(|| Error("missing pprof inline frames".into()))?
            .iter()
            .map(|l| {
                l["function_id"]
                    .as_u64()
                    .ok_or_else(|| Error("missing pprof frame function".into()))
            })
            .collect::<Result<Vec<_>>>()?;
        if lines.iter().any(|id| !functions.contains_key(id))
            || locations.insert(id, lines).is_some()
        {
            return Err(Error("invalid pprof inline frame reference".into()));
        }
    }
    let mut unresolved_self_value = 0i64;
    for row in records.iter().filter(|r| r.schema == "pprof.sample") {
        let values = row.payload["values"]
            .as_array()
            .ok_or_else(|| Error("missing pprof sample values".into()))?;
        if values.len() != types.len() {
            return Err(Error("pprof sample value cardinality changed".into()));
        }
        let value = values[index]
            .as_i64()
            .ok_or_else(|| Error("invalid pprof sample value".into()))?;
        let stack = row.payload["location_ids"]
            .as_array()
            .ok_or_else(|| Error("missing pprof sample stack".into()))?;
        let mut inclusive = BTreeSet::new();
        let mut leaf = None;
        for (ordinal, location) in stack.iter().enumerate() {
            let id = location
                .as_u64()
                .ok_or_else(|| Error("invalid pprof stack id".into()))?;
            let frames = locations
                .get(&id)
                .ok_or_else(|| Error("unknown pprof stack location".into()))?;
            if ordinal == 0 {
                leaf = frames.first().copied();
            }
            inclusive.extend(frames.iter().copied());
        }
        if let Some(id) = leaf {
            let cost = functions.get_mut(&id).unwrap();
            cost.self_value = cost
                .self_value
                .checked_add(value)
                .ok_or_else(|| Error("pprof self aggregate overflow".into()))?;
        } else {
            unresolved_self_value = unresolved_self_value
                .checked_add(value)
                .ok_or_else(|| Error("pprof unresolved aggregate overflow".into()))?;
        }
        for id in inclusive {
            let cost = functions.get_mut(&id).unwrap();
            cost.inclusive_value = cost
                .inclusive_value
                .checked_add(value)
                .ok_or_else(|| Error("pprof inclusive aggregate overflow".into()))?;
        }
    }
    Ok(Costs {
        value_type: value_type.into(),
        unit: unit.into(),
        functions: functions.into_values().collect(),
        unresolved_self_value,
    })
}

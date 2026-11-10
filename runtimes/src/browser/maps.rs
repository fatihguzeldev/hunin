use super::BrowserExtraction;
use crate::{Error, Extraction, Limits, Result, record, schema, source_maps::BoundMap};
use hunin_core::records::FieldType;
use serde_json::json;

pub fn project_safari(
    map: &BoundMap,
    stream: &super::BrowserStream,
    subject: &str,
    limits: Limits,
) -> Result<Extraction> {
    let scripts: Vec<_> = stream
        .extraction
        .records
        .iter()
        .filter(|r| {
            r.schema == "safari.script"
                && r.payload["original_sha256"] == map.binding().generated_sha256
        })
        .collect();
    if scripts.len() != 1 {
        return Err(Error(
            "Safari map requires one matching compiler-bound original script".into(),
        ));
    }
    let script = &scripts[0].payload["id"];
    let observed: std::collections::BTreeSet<_> = stream
        .extraction
        .records
        .iter()
        .filter(|r| r.schema == "safari.call")
        .filter_map(|r| r.payload["function_id"].as_str())
        .collect();
    let mut out = Extraction::new(
        limits,
        json!({"interpretation":"safari-instrumented-declaration-source-map-v1","generated_source":stream.key,"generated_sha256":map.binding().generated_sha256,"map_sha256":map.binding().map_sha256,"position_meaning":"compiler-observed original JS declaration; only functions with observed body events are projected"}),
    );
    out.schemas = vec![schema(
        "browser.sourcemap-location",
        "original source location derived from the instrumented script's bound pre-transform source",
        &[
            (
                "generated_record",
                FieldType::String,
                None,
                "source-local safari.function record id",
            ),
            (
                "generated_source",
                FieldType::String,
                None,
                "stream owning the declaration and call events",
            ),
            (
                "source",
                FieldType::String,
                None,
                "original source map source",
            ),
            (
                "line",
                FieldType::Unsigned,
                None,
                "zero-based original line",
            ),
            (
                "column",
                FieldType::Unsigned,
                None,
                "zero-based utf-16 original column",
            ),
        ],
    )];
    for row in &stream.extraction.records {
        if row.schema != "safari.function"
            || row.payload["script"] != *script
            || !row.payload["id"]
                .as_str()
                .is_some_and(|id| observed.contains(id))
        {
            continue;
        }
        let line = row.payload["line"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| Error("Safari generated line out of range".into()))?;
        let column = row.payload["column"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| Error("Safari generated column out of range".into()))?;
        let Some(original) = map.lookup(line, column) else {
            continue;
        };
        let mut projected = record(
            format!("original:{}", row.id),
            "browser.sourcemap-location",
            subject,
            json!({"generated_record":row.id,"generated_source":stream.key,"generated_sha256":map.binding().generated_sha256,"map_sha256":map.binding().map_sha256,"source":original.source,"line":original.line,"column":original.column,"function_id":row.payload["id"]}),
        );
        projected.function = original.name.or_else(|| row.function.clone());
        projected.location = Some(format!(
            "{}:{}:{}",
            original.source,
            u64::from(original.line) + 1,
            u64::from(original.column) + 1
        ));
        out.push(projected)?;
    }
    Ok(out)
}

/// maps declaration coordinates only when debugger bytes and one profile window agree.
/// the returned records preserve references to the unchanged process-local frame rows.
pub fn project_gecko(
    map: &BoundMap,
    browser: &BrowserExtraction,
    script_url: &str,
    subject: &str,
    limits: Limits,
) -> Result<Extraction> {
    let descriptors: Vec<_> = browser
        .collection
        .records
        .iter()
        .filter(|r| r.schema == "browser.script" && r.payload["metadata"]["url"] == script_url)
        .collect();
    if descriptors.len() != 1 {
        return Err(Error(
            "gecko mapping needs one debugger script identity".into(),
        ));
    }
    let descriptor = descriptors[0];
    if descriptor.payload["sha256"].as_str() != Some(&map.binding().generated_sha256) {
        return Err(Error(
            "gecko executed script does not match map build".into(),
        ));
    }
    let page = descriptor.payload["metadata"]["page_url"]
        .as_str()
        .ok_or_else(|| Error("debugger page binding missing".into()))?;
    let mut windows = Vec::new();
    for stream in &browser.streams {
        if stream.extraction.metadata["format"] != "gecko-36" {
            return Err(Error(
                "gecko mapping requires gecko36 source streams".into(),
            ));
        }
        for row in &stream.extraction.records {
            if row.schema == "gecko.metadata"
                && row.payload["owner"] == "process"
                && row.payload["field"] == "pages"
            {
                let page_info = &row.payload["value"]["data"];
                if page_info["url"] == page {
                    let window = page_info["innerWindowID"]
                        .as_u64()
                        .ok_or_else(|| Error("profile window identity missing".into()))?;
                    windows.push((stream, window));
                }
            }
        }
    }
    if windows.len() != 1 {
        return Err(Error(
            "debugger page URL does not identify one profile window; mapping withheld".into(),
        ));
    }
    let (stream, window) = windows[0];
    let mut window_columns = std::collections::BTreeMap::new();
    for layout in &stream.extraction.records {
        if layout.schema != "gecko.table-schema" || layout.payload["table"] != "frameTable" {
            continue;
        }
        let thread = layout.payload["thread"]
            .as_str()
            .ok_or_else(|| Error("gecko frame thread missing".into()))?;
        let column = layout.payload["schema"]["innerWindowID"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| Error("gecko frame window column missing".into()))?;
        if window_columns.insert(thread, column).is_some() {
            return Err(Error("duplicate gecko frame layout".into()));
        }
    }
    let mut result = Extraction::new(
        limits,
        json!({"interpretation":"gecko36-declaration-source-map-v1","generated_sha256":map.binding().generated_sha256,"map_sha256":map.binding().map_sha256,"process_stream":stream.key,"inner_window_id":window,"context":descriptor.payload["metadata"]["context"],"position_meaning":"function declaration in gecko36 location string, not sampled instruction"}),
    );
    result.schemas = vec![schema(
        "browser.sourcemap-location",
        "original function declaration derived from debugger-bound script and process-local frame",
        &[
            (
                "generated_record",
                FieldType::String,
                None,
                "source-local gecko frame row id",
            ),
            (
                "generated_source",
                FieldType::String,
                None,
                "process stream owning generated record",
            ),
            (
                "source",
                FieldType::String,
                None,
                "original source map source",
            ),
            (
                "line",
                FieldType::Unsigned,
                None,
                "zero-based original line",
            ),
            (
                "column",
                FieldType::Unsigned,
                None,
                "zero-based utf-16 original column",
            ),
        ],
    )];
    for row in &stream.extraction.records {
        if row.schema != "gecko.table-row" || row.payload["table"] != "frameTable" {
            continue;
        }
        let thread = row.payload["thread"]
            .as_str()
            .ok_or_else(|| Error("gecko frame thread missing".into()))?;
        let column = *window_columns
            .get(thread)
            .ok_or_else(|| Error("gecko frame layout missing".into()))?;
        if row.payload["values"][column].as_u64() != Some(window) {
            continue;
        }
        let Some(coordinates) = row
            .location
            .as_deref()
            .and_then(|p| p.strip_prefix(script_url))
            .and_then(|p| p.strip_prefix(':'))
        else {
            continue;
        };
        let Some((line, column)) = coordinates.split_once(':') else {
            continue;
        };
        let Some(line) = line.parse::<u32>().ok().and_then(|v| v.checked_sub(1)) else {
            continue;
        };
        let Some(column) = column.parse::<u32>().ok().and_then(|v| v.checked_sub(1)) else {
            continue;
        };
        let Some(original) = map.lookup(line, column) else {
            continue;
        };
        let mut projection = record(
            format!("original:{}", row.id),
            "browser.sourcemap-location",
            subject,
            json!({"generated_record":row.id,"generated_source":stream.key,"generated_sha256":map.binding().generated_sha256,"map_sha256":map.binding().map_sha256,"inner_window_id":window,"generated_line":line,"generated_column":column,"source":original.source,"line":original.line,"column":original.column}),
        );
        projection.function = original.name.or_else(|| row.function.clone());
        projection.location = Some(format!(
            "{}:{}:{}",
            original.source,
            u64::from(original.line) + 1,
            u64::from(original.column) + 1
        ));
        result.push(projection)?;
    }
    Ok(result)
}

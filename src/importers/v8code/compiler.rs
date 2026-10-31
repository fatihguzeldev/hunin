//! Join exact same-run compiler origins; missing links never gain nearest origins.
use super::{
    compiler_native, evidence,
    normalize::{NativeUnit, WasmUnit},
};
pub(super) use super::{compiler_native::relocated_words_match, compiler_source::source_matches};
use crate::{
    importers::output::RecordBatch,
    model::code::{Cardinality, CodeMapping, CodeRange, MappingProof, RangeSpace, Verification},
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

// Serde rejects duplicate envelope fields instead of merging interleaved exports.
#[derive(Deserialize, Serialize)]
struct TraceDocument {
    function: Value,
    source: Value,
    #[serde(rename = "sourceLineToBytecodePosition")]
    positions: Value,
    phases: Vec<Value>,
    #[serde(default)]
    sources: Value,
    #[serde(default)]
    inlinings: Value,
}

fn origin(nodes: &BTreeMap<u64, Value>, id: u64, seen: &mut BTreeSet<u64>) -> Option<u64> {
    if !seen.insert(id) || seen.len() > 256 {
        return None;
    }
    let value = &nodes.get(&id)?["origin"];
    value["bytecodePosition"].as_u64().or_else(|| {
        value["nodeId"]
            .as_u64()
            .and_then(|id| origin(nodes, id, seen))
    })
}
type Origins = BTreeMap<u64, BTreeSet<u64>>;

pub(super) fn selected_origins(
    trace: &Value,
    functions: &[&WasmUnit],
    records: &mut RecordBatch<'_>,
) -> Result<Option<Origins>> {
    let Some(phases) = trace["phases"].as_array() else {
        return Ok(None);
    };
    let Some(selection) = phases
        .iter()
        .find(|p| p["nodeIdToInstructionRange"].is_object())
        .and_then(|p| p["nodeIdToInstructionRange"].as_object())
    else {
        return Ok(None);
    };
    // Reject a tiny metadata file that asks for disproportionate inversion work.
    let mut expanded = 0u64;
    for range in selection.values() {
        records.check()?;
        let (Some(start), Some(end)) = (range[0].as_u64(), range[1].as_u64()) else {
            return Ok(None);
        };
        let Some(size) = end.checked_sub(start) else {
            return Ok(None);
        };
        expanded = expanded.saturating_add(size);
        if size > 100_000 || expanded > 2_000_000 {
            return Ok(None);
        }
    }
    let mut nodes = BTreeMap::new();
    for phase in phases {
        records.check()?;
        if phase["type"] == "graph" {
            let Some(rows) = phase["data"]["nodes"].as_array() else {
                return Ok(None);
            };
            for node in rows {
                records.frame()?;
                let Some(id) = node["id"].as_u64() else {
                    return Ok(None);
                };
                nodes.insert(id, node.clone());
            }
        }
    }
    let mut chosen: Origins = BTreeMap::new();
    for (node, range) in selection {
        records.check()?;
        let Ok(node) = node.parse() else {
            return Ok(None);
        };
        let mut seen = BTreeSet::new();
        let position = origin(&nodes, node, &mut seen);
        for _ in seen {
            records.frame()?;
        }
        if let Some(position) = position
            && functions
                .iter()
                .all(|unit| unit.function.operations.contains_key(&position))
        {
            for id in range[0].as_u64().unwrap()..range[1].as_u64().unwrap() {
                records.frame()?;
                chosen.entry(id).or_default().insert(position);
            }
        }
    }
    Ok(Some(chosen))
}

fn intervals(trace: &Value, chosen: &Origins) -> Option<Vec<(u64, u64, Vec<u64>)>> {
    let phases = trace["phases"].as_array()?;
    if phases
        .iter()
        .filter(|p| p["nodeIdToInstructionRange"].is_object())
        .count()
        != 1
        || phases
            .iter()
            .filter(|p| p["instructionOffsetToPCOffset"].is_object())
            .count()
            != 1
        || phases
            .iter()
            .filter(|p| p["name"] == "after register allocation")
            .count()
            != 1
        || phases.iter().filter(|p| p["type"] == "disassembly").count() != 1
        || trace["inlinings"]
            .as_object()
            .is_some_and(|inlinings| !inlinings.is_empty())
    {
        return None;
    }
    let generation = phases
        .iter()
        .find(|p| p["instructionOffsetToPCOffset"].is_object())?;
    let offsets = generation["instructionOffsetToPCOffset"].as_object()?;
    let sequence = phases
        .iter()
        .find(|p| p["name"] == "after register allocation")?;
    let mut instructions = BTreeMap::new();
    for block in sequence["blocks"].as_array()? {
        for instruction in block["instructions"].as_array()? {
            instructions.insert(instruction["id"].as_u64()?, instruction);
        }
    }
    let mut gaps = offsets
        .values()
        .filter_map(|p| p["gap"].as_u64())
        .collect::<BTreeSet<_>>();
    if let Some(end) = generation["codeOffsetsInfo"]["outOfLineCode"].as_u64() {
        gaps.insert(end);
    }
    let mut ranges = vec![];
    for (id, position) in offsets {
        let id = id.parse::<u64>().ok()?;
        let instruction = instructions.get(&id)?;
        let gap = position["gap"].as_u64()?;
        let arch = position["arch"].as_u64()?;
        let condition = position["condition"].as_u64()?;
        if gap > arch || arch > condition {
            return None;
        }
        let Some(origins) = chosen.get(&id).filter(|o| !o.is_empty()) else {
            continue;
        };
        if arch < condition {
            ranges.push((arch, condition, origins.iter().copied().collect()));
        }
        if !instruction["flags"].as_str().unwrap_or("").is_empty()
            && let Some(&end) = gaps
                .range((std::ops::Bound::Excluded(gap), std::ops::Bound::Unbounded))
                .next()
            && condition < end
        {
            ranges.push((condition, end, origins.iter().copied().collect()));
        }
    }
    Some(ranges)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn enrich(
    parent: &Path,
    manifest: &Value,
    run: &str,
    key: &str,
    pid: u64,
    architecture: &str,
    maximum: u64,
    native: &[NativeUnit],
    wasm: &[WasmUnit],
    records: &mut RecordBatch<'_>,
    diagnostics: &mut Vec<String>,
) -> Result<()> {
    let exports = manifest["compiler_exports"]
        .as_array()
        .context("compiler inventory missing")?;
    if exports.is_empty() {
        return Ok(());
    }
    if architecture != "arm64" {
        diagnostics.push(format!("Compiler byte/relocation join is not supported for {architecture}; exported metadata is retained."));
        return Ok(());
    }
    let Some(published_name) = manifest["published_code"]["path"].as_str() else {
        diagnostics
            .push("Compiler exports have no retained published-code relocation evidence.".into());
        return Ok(());
    };
    let published_bytes = evidence::read(parent, published_name, maximum)?;
    let published_reference =
        evidence::reference(published_name, &published_bytes, "profile-sidecar");
    let published = compiler_native::published(std::str::from_utf8(&published_bytes)?);
    let mut by_address: BTreeMap<u64, Vec<&NativeUnit>> = BTreeMap::new();
    for unit in native {
        by_address.entry(unit.address).or_default().push(unit);
    }
    let mut mappings = 0;
    let mut documents = 0;
    let mut dedup = BTreeSet::new();
    for file in exports {
        records.check()?;
        let name = file["path"]
            .as_str()
            .context("compiler export filename missing")?;
        let bytes = evidence::read(parent, name, maximum)?;
        let reference = evidence::reference(name, &bytes, "profile-sidecar");
        // V8 can append multiple complete JSON documents to one function filename.
        let mut stream = serde_json::Deserializer::from_slice(&bytes).into_iter::<TraceDocument>();
        let mut start = 0;
        while let Some(document) = stream.next() {
            records.check()?;
            let trace = match document {
                Ok(trace) => serde_json::to_value(trace)?,
                Err(_) => {
                    diagnostics.push(format!("Compiler export {name} has an incomplete or unsupported JSON suffix; complete preceding documents remain usable."));
                    break;
                }
            };
            documents += 1;
            let mut compiler_reference = reference.clone();
            compiler_reference.offset = Some(start as u64);
            compiler_reference.length = Some((stream.byte_offset() - start) as u64);
            start = stream.byte_offset();
            let Some(phases) = trace["phases"].as_array() else {
                diagnostics.push(format!(
                    "Compiler document in {name} has no supported phases."
                ));
                continue;
            };
            let Some(disassembly) = phases
                .iter()
                .find(|p| p["type"] == "disassembly")
                .and_then(|p| p["data"].as_str())
            else {
                continue;
            };
            let temporary = compiler_native::words(disassembly);
            let mut linked = false;
            for published in &published {
                let Some(units) = by_address.get(&published.address) else {
                    continue;
                };
                let candidates = wasm
                    .iter()
                    .filter(|unit| unit.function.index == published.function_index)
                    .map(|unit| {
                        source_matches(&trace, &unit.function).map(|matches| (unit, matches))
                    })
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .filter_map(|(unit, matches)| matches.then_some(unit))
                    .collect::<Vec<_>>();
                if candidates.len() != 1 {
                    continue;
                }
                let Some(chosen) = selected_origins(&trace, &candidates, records)? else {
                    continue;
                };
                let Some(ranges) = intervals(&trace, &chosen) else {
                    continue;
                };
                for native in units {
                    if native.content.len() != published.body_size {
                        continue;
                    }
                    let Some(instructions) = native.content.get(..published.instruction_size)
                    else {
                        continue;
                    };
                    if !relocated_words_match(
                        &temporary,
                        &published.words,
                        &published.relocations,
                        instructions,
                    ) {
                        continue;
                    }
                    linked = true;
                    let target = candidates[0];
                    let mut segments: Vec<(u64, u64, BTreeSet<u64>)> = vec![];
                    for (&offset, &word) in &published.words {
                        if word == 0xd503201f {
                            continue;
                        }
                        let origins = ranges
                            .iter()
                            .filter(|(lo, hi, _)| *lo <= offset && offset + 4 <= *hi)
                            .flat_map(|(_, _, origins)| origins.iter().copied())
                            .collect::<BTreeSet<_>>();
                        if origins.is_empty() {
                            continue;
                        }
                        if let Some(last) = segments
                            .last_mut()
                            .filter(|last| last.1 == offset && last.2 == origins)
                        {
                            last.1 = offset + 4;
                        } else {
                            segments.push((offset, offset + 4, origins));
                        }
                    }
                    for (lo, hi, origins) in segments {
                        for &position in &origins {
                            if !dedup.insert((
                                native.id.clone(),
                                target.id.clone(),
                                lo,
                                hi,
                                position,
                            )) {
                                continue;
                            }
                            let end = target.function.operations[&position].0;
                            let mapping=CodeMapping {schema_version:1,
                            from_range:CodeRange {space:RangeSpace::RuntimeAddress,start:format!("0x{:x}",native.address+lo),end_exclusive:format!("0x{:x}",native.address+hi),function_index:None},
                            to_range:CodeRange {space:RangeSpace::FunctionOffset,start:format!("0x{position:x}"),end_exclusive:format!("0x{end:x}"),function_index:Some(target.function.index)},
                            cardinality:if origins.len()==1{Cardinality::Exclusive}else{Cardinality::Shared},validity:Some(native.validity.clone()),
                            proof:MappingProof {artifact_refs:vec![{let mut proof=native.reference.clone();proof.length=Some(published.instruction_size as u64);proof},published_reference.clone(),compiler_reference.clone(),target.reference.clone()],
                                method:"v8_compiler_own_origin; exact_wasm_body_and_published_bytes; declared_arm64_call_relocations".into(),verification:Verification::IdentityChecked,validator:None,semantic_evidence:vec![]}};
                            mapping.validate()?;
                            records.push(json!({"schema_version":1,"kind":"relation","id":format!("code:{key}:mapping:{mappings}"),"run_id":run,
                            "producer_id":format!("v8-code:{key}:{pid}"),"pid":pid,"tid":native.validity.clock,"timestamp_us":native.validity.start_us.unwrap_or(0),
                            "basis":"observed","from":native.id,"to":target.id,"relation":"code_maps_to","evidence":[native.id,target.id],
                            "attributes":{"clock":native.validity.clock,"clock_alignment":"runtime_relative","code_mapping":mapping}}))?;
                            mappings += 1;
                        }
                    }
                }
            }
            if !linked {
                diagnostics.push(format!("Compiler document in {name} has no unique verified native/module join or lacks required node/instruction/origin fields."));
            }
        }
    }
    diagnostics.push(format!("Read {documents} compiler documents and retained {mappings} exact native-to-Wasm origin ranges. Register gaps, non-opcode origins, missing/ambiguous exports and padding have no invented ownership."));
    diagnostics.push("Compiler inventory contains the files available at capture end; completeness across every compilation is not established.".into());
    Ok(())
}

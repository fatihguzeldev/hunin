use super::{compiler, evidence, ll, raw, wasm};
use crate::{
    importers::output::{RecordBatch, sample},
    model::{
        code::{
            ArtifactRef, CodeInstance, CodeRange, CodeValidity, InstanceKind, RangeSpace,
            Representation,
        },
        observations::{SampleWeightUnit, StackSample},
    },
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

pub(super) struct WasmUnit {
    pub id: String,
    pub reference: ArtifactRef,
    pub function: wasm::Function,
}
pub(super) struct NativeUnit {
    pub id: String,
    pub address: u64,
    pub content: Arc<[u8]>,
    pub reference: ArtifactRef,
    pub validity: CodeValidity,
}
fn event(
    run: &str,
    key: &str,
    pid: u64,
    time: u64,
    id: &str,
    clock: &str,
    instance: CodeInstance,
) -> Result<Value> {
    instance.validate()?;
    Ok(
        json!({"schema_version":1,"kind":"event","id":id,"run_id":run,
        "producer_id":format!("v8-code:{key}:{pid}"),"pid":pid,"tid":clock,"timestamp_us":time,
        "basis":"observed","name":"code.instance","attributes":{"clock":clock,"clock_alignment":"runtime_relative","code_instance":instance}}),
    )
}
pub(super) fn decode_into(
    path: &Path,
    manifest: &Value,
    run: &str,
    key: &str,
    records: &mut RecordBatch<'_>,
) -> Result<Vec<String>> {
    let sidecars = evidence::sidecar_paths(path, manifest)?;
    let parent = path.parent().context("V8 manifest parent missing")?;
    let maximum = manifest["max_artifact_bytes"]
        .as_u64()
        .unwrap_or(67_108_864);
    ensure!(
        (1..=536_870_912).contains(&maximum),
        "invalid V8 code artifact budget"
    );
    let mut total = 0u64;
    for sidecar in &sidecars {
        total = total
            .checked_add(std::fs::metadata(sidecar)?.len())
            .context("V8 inventory size overflows")?;
        ensure!(
            total <= maximum,
            "V8 code inventory exceeds declared artifact budget"
        );
    }
    let pid = manifest["pid"].as_u64().context("V8 code PID missing")?;
    let architecture = manifest["runtime"]["arch"]
        .as_str()
        .context("V8 architecture missing")?;
    let mut diagnostics = manifest["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    diagnostics.push("V8 native ticks are sample counts in their own isolate-relative clock; they are not matched to CPU-profile samples or hardware cycles.".into());
    diagnostics.push("Wasm custody is a post-workload snapshot of still-live modules; forced exit and collected modules can leave it incomplete.".into());
    let mut wasm_units = vec![];
    let mut wasm_seen = BTreeSet::new();
    for row in manifest["wasm"].as_array().unwrap() {
        records.check()?;
        let name = row["path"].as_str().context("Wasm filename missing")?;
        let bytes = evidence::read(parent, name, maximum)?;
        let reference = evidence::reference(name, &bytes, "profile-sidecar");
        ensure!(
            row["sha256"].as_str() == Some(reference.sha256.as_str()),
            "retained Wasm digest disagrees with runtime custody descriptor"
        );
        if !wasm_seen.insert(reference.sha256.clone()) {
            continue;
        }
        for function in wasm::functions(&bytes)? {
            let id = format!("code:{key}:wasm:{}:{}", reference.sha256, function.index);
            let clock = format!("v8:{run}:{pid}:wasm-metadata");
            let instance = CodeInstance {
                schema_version: 1,
                instance_kind: InstanceKind::Static,
                runtime_instance_id: None,
                representation: Representation::Wasm,
                architecture: None,
                code_id: Some(format!("sha256:{}", reference.sha256)),
                artifact: Some(reference.clone()),
                range: Some(CodeRange {
                    space: RangeSpace::FunctionOffset,
                    start: "0x0".into(),
                    end_exclusive: format!("0x{:x}", function.body.len()),
                    function_index: Some(function.index),
                }),
                validity: None,
                label: Some(function.label.clone()),
                source: None,
            };
            let mut declaration = event(run, key, pid, 0, &id, &clock, instance)?;
            declaration["attributes"]["wasm_module"] = json!({"function_body_offset":function.body_start,"script_id":row["script_id"],"url":row["url"]});
            records.push(declaration)?;
            wasm_units.push(WasmUnit {
                id,
                reference: reference.clone(),
                function,
            });
        }
    }
    let mut sample_index = 0usize;
    let mut native_units = vec![];
    for (log_index, row) in manifest["logs"].as_array().unwrap().iter().enumerate() {
        records.check()?;
        let name = row["path"].as_str().context("V8 log filename missing")?;
        let bytes = evidence::read(parent, name, maximum)?;
        let log_reference = evidence::reference(name, &bytes, "profile-sidecar");
        let log = raw::parse_controlled(std::str::from_utf8(&bytes)?, |visit| match visit {
            raw::Visit::Record => records.check(),
            raw::Visit::Frame => records.frame(),
        })?;
        ensure!(
            manifest["runtime"]["v8"].as_str() == Some(log.version.as_str()),
            "V8 log version disagrees with runtime manifest"
        );
        let clock = format!("v8:{run}:{pid}:{name}:{}", &log_reference.sha256[..16]);
        diagnostics.extend(log.diagnostics);
        let (ll_records, ll_reference) = if let Some(name) = row["ll_path"].as_str() {
            let bytes = evidence::read(parent, name, maximum)?;
            let reference = evidence::reference(name, &bytes, "profile-sidecar");
            if matches!(architecture, "arm64" | "x64") {
                (ll::parse(&bytes, architecture)?, Some(reference))
            } else {
                diagnostics.push(format!("V8 low-level code ABI {architecture} is not supported; raw bytes remain retained."));
                (vec![], None)
            }
        } else {
            diagnostics.push(format!("V8 log {name} has no low-level code-byte export."));
            (vec![], None)
        };
        let mut by_range: BTreeMap<(u64, u64), Vec<&ll::Bytes>> = BTreeMap::new();
        for bytes in &ll_records {
            by_range
                .entry((bytes.address, bytes.content.len() as u64))
                .or_default()
                .push(bytes);
        }
        let mut shared_bytes: BTreeMap<u64, Arc<[u8]>> = BTreeMap::new();
        let mut generations_by_range: BTreeMap<(u64, u64), usize> = BTreeMap::new();
        for code in log
            .codes
            .iter()
            .filter(|code| code.origin == code.generation)
        {
            *generations_by_range
                .entry((code.address, code.size))
                .or_default() += 1;
        }
        let mut content: BTreeMap<usize, Arc<[u8]>> = BTreeMap::new();
        let mut ids = vec![];
        let mut code_ids = vec![];
        for code in &log.codes {
            records.check()?;
            let id = format!("code:{key}:native:{log_index}:{}", code.generation);
            ids.push(id.clone());
            let origin = &log.codes[code.origin];
            let candidates = by_range
                .get(&(origin.address, origin.size))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            // A move record exports no new bytes; relocation may patch them.
            let matched = candidates.first().filter(|first| {
                code.origin == code.generation
                    && generations_by_range.get(&(code.address, code.size)) == Some(&1)
                    && candidates
                        .iter()
                        .all(|other| other.content == first.content)
            });
            let mut proof = log_reference.clone();
            let code_id = if let (Some(bytes), Some(reference)) = (matched, ll_reference.as_ref()) {
                proof = reference.clone();
                proof.offset = Some(bytes.offset);
                proof.length = Some(bytes.content.len() as u64);
                let shared = shared_bytes
                    .entry(bytes.offset)
                    .or_insert_with(|| Arc::from(bytes.content.clone()))
                    .clone();
                content.insert(code.generation, shared);
                Some(format!("sha256:{:x}", Sha256::digest(&bytes.content)))
            } else {
                None
            };
            let validity = CodeValidity {
                clock: clock.clone(),
                alignment: "runtime_relative".into(),
                start_us: Some(code.start_us),
                end_us: (code.end_us <= 9_007_199_254_740_991).then_some(code.end_us),
                start_censored: code.start_censored,
                end_censored: code.end_censored,
                boundary_basis:
                    "v8_log_event_order; untimed_move_delete_and_capture_end_are_censored".into(),
            };
            code_ids.push(code_id.clone());
            let instance = CodeInstance {
                schema_version: 1,
                instance_kind: InstanceKind::Runtime,
                runtime_instance_id: Some(format!("{clock}:{}", code.generation)),
                representation: if code.bytecode {
                    Representation::Bytecode
                } else {
                    Representation::MachineCode
                },
                architecture: Some(architecture.into()),
                code_id,
                artifact: Some(proof.clone()),
                range: Some(CodeRange {
                    space: RangeSpace::RuntimeAddress,
                    start: format!("0x{:x}", code.address),
                    end_exclusive: format!("0x{:x}", code.address + code.size),
                    function_index: None,
                }),
                validity: Some(validity.clone()),
                label: Some(code.name.clone()),
                source: None,
            };
            let mut declaration = event(run, key, pid, code.start_us, &id, &clock, instance)?;
            declaration["attributes"]["v8_log_line"] = json!(code.create_line);
            if let Some(bytes) = matched {
                declaration["attributes"]["v8_byte_export_label"] = json!(bytes.name);
            }
            records.push(declaration)?;
            if !code.bytecode
                && let Some(bytes) = content.get(&code.generation)
            {
                native_units.push(NativeUnit {
                    id,
                    address: code.address,
                    content: bytes.clone(),
                    reference: proof,
                    validity,
                });
            }
        }
        for tick in log.ticks {
            records.check()?;
            let mut frames = vec![];
            let mut provenance = vec![];
            for (index, frame) in tick.frames.iter().enumerate() {
                records.frame()?;
                let candidate = if index == 0 {
                    frame.code
                } else if architecture == "arm64" {
                    frame.arm64_callsite_code
                } else {
                    None
                };
                let coordinate = if index == 0 {
                    Some(frame.address)
                } else if architecture == "arm64" {
                    candidate.and_then(|code| {
                        content.get(&code).and_then(|bytes| {
                            let call = frame.address.checked_sub(4)?;
                            let offset = call.checked_sub(log.codes[code].address)? as usize;
                            if !call.is_multiple_of(4) || !offset.is_multiple_of(4) {
                                return None;
                            }
                            let word =
                                u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?);
                            ((word & 0xfc000000 == 0x94000000) || (word & 0xfffffc1f == 0xd63f0000))
                                .then_some(call)
                        })
                    })
                } else {
                    None
                };
                let code = if index == 0 || coordinate.is_some() {
                    candidate
                } else {
                    None
                };
                let mut normalized = json!({"function":code.map(|i|log.codes[i].name.as_str()).unwrap_or(if index==0{"[unresolved native PC]"}else{"[unresolved return address]"}),"address":format!("0x{:x}",frame.address)});
                if let Some(code_id) = code.and_then(|code| code_ids[code].as_ref()) {
                    normalized["code_id"] = json!(code_id);
                }
                if let (Some(code), Some(coordinate)) = (code, coordinate) {
                    normalized["code_instance_id"] = json!(ids[code]);
                    normalized["code_offset"] = json!(format!("0x{coordinate:x}"));
                    normalized["code_address_basis"] = json!(if index == 0 {
                        "runtime_pc"
                    } else {
                        "validated_arm64_callsite_from_return_address"
                    });
                }
                provenance.push(json!({"reported_address":format!("0x{:x}",frame.address),"basis":if index==0{"sampled_pc"}else if coordinate.is_some(){"validated_arm64_callsite_from_return_address"}else{"reported_return_address_unmapped"}}));
                frames.push(normalized);
            }
            frames.reverse();
            provenance.reverse();
            records.push(sample(run,key,sample_index,StackSample{pid,tid:name,timestamp_us:tick.timestamp_us,
                stack:frames,weight:1.0,weight_unit:SampleWeightUnit::Samples,
                attributes:json!({"source_format":"v8-code","clock":clock,"clock_alignment":"runtime_relative","v8_log_line":tick.line,"stack_overflow":tick.overflow,"frame_coordinate_provenance":provenance})}))?;
            sample_index += 1;
        }
    }
    compiler::enrich(
        parent,
        manifest,
        run,
        key,
        pid,
        architecture,
        maximum,
        &native_units,
        &wasm_units,
        records,
        &mut diagnostics,
    )?;
    if sample_index == 0 {
        diagnostics.push("V8 code capture contains no native tick samples.".into());
    }
    Ok(diagnostics)
}

//! Bounded code mapping inspection over one selected-record transaction.
use super::WorkBudget;
use crate::model::code::{
    self, ArtifactRef, Cardinality, CodeInstance, CodeMapping, CodeRange, CodeRecord,
    CodeUnavailable, InstanceKind, UnavailableState,
};
use crate::storage::QueryReadPort;
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
mod association;
use association::Associations;

struct Instance {
    record: Value,
    data: CodeInstance,
}
struct Mapping {
    record: Value,
    data: CodeMapping,
}
#[derive(Default)]
struct Bucket {
    count: u64,
    weight: f64,
}
impl Bucket {
    fn add(&mut self, weight: f64) {
        self.count += 1;
        self.weight += weight;
    }
    fn value(&self, available: bool) -> Value {
        json!({"sample_count":self.count,"weight":available.then_some(self.weight)})
    }
}

fn instance_at(instance: &CodeInstance, time: u64, attributes: &Value) -> Option<bool> {
    if instance.instance_kind == InstanceKind::Static {
        Some(true)
    } else {
        instance
            .validity
            .as_ref()
            .and_then(|validity| validity.at_in(time, attributes))
    }
}

fn proof_status(
    port: &mut QueryReadPort<'_>,
    reference: &ArtifactRef,
    artifacts: &mut BTreeMap<(String, String), Value>,
    work: &mut WorkBudget,
) -> Result<bool> {
    let key = (reference.artifact_id.clone(), reference.sha256.clone());
    if !artifacts.contains_key(&key) {
        work.entry()?;
        let metadata = port.artifact_metadata(&reference.artifact_id)?;
        let matches = metadata.sha256 == reference.sha256;
        artifacts.insert(
            key.clone(),
            json!({
                "artifact_id":reference.artifact_id,
                "sha256":reference.sha256,
                "metadata_sha256":metadata.sha256,
                "size":metadata.size,
            "name":metadata.name,
            "format":metadata.format,
                "status":if matches {"metadata_matches"} else {"mismatch"}
            }),
        );
    }
    let status = &artifacts[&key];
    Ok(status["status"] == "metadata_matches"
        && reference
            .offset
            .zip(reference.length)
            .is_none_or(|(offset, length)| {
                offset
                    .checked_add(length)
                    .is_some_and(|end| end <= status["size"].as_u64().unwrap_or(0))
            }))
}

pub(super) fn inspect(
    port: &mut QueryReadPort<'_>,
    params: &Value,
    work: &mut WorkBudget,
) -> Result<Value> {
    let mut records = Vec::new();
    port.visit(&["event", "relation", "sample"], |text| {
        work.check()?;
        records.push(serde_json::from_str::<Value>(text)?);
        Ok(())
    })?;
    let clock_domain = params["clock_domain"].as_str();
    let placed = |record: &Value| match clock_domain {
        Some(domain) => {
            crate::model::clocks::runtime_relative_domain(&record["attributes"]) == Some(domain)
        }
        None => crate::model::clocks::inspect(&record["attributes"]).aligned,
    };
    let requested = params["observation_id"].as_str().unwrap();
    let index = params["frame_index"].as_u64().unwrap_or(0) as usize;
    let limit = params["limit"].as_u64().unwrap_or(100).min(500) as usize;
    let raw_anchor = records.iter().find(|record| record["id"] == requested);
    let mut instances = BTreeMap::new();
    let mut mappings = BTreeMap::new();
    let mut associations = Associations::default();
    let mut unavailable: BTreeMap<String, (Value, CodeUnavailable)> = BTreeMap::new();
    let mut unavailable_by_anchor = HashMap::new();
    let mut diagnostics = vec![];
    for record in &records {
        work.check()?;
        if record["kind"] == "sample" {
            continue;
        }
        match code::parse_record(record)? {
            Some(CodeRecord::Instance(data)) => {
                work.entry()?;
                instances.insert(
                    record["id"].as_str().unwrap().to_owned(),
                    Instance {
                        record: record.clone(),
                        data,
                    },
                );
            }
            Some(CodeRecord::Mapping(data)) => {
                work.entry()?;
                mappings.insert(
                    record["id"].as_str().unwrap().to_owned(),
                    Mapping {
                        record: record.clone(),
                        data,
                    },
                );
            }
            Some(CodeRecord::FrameMapping(data)) => {
                work.entry()?;
                associations.insert(record, data);
            }
            Some(CodeRecord::Unavailable(data)) => {
                work.entry()?;
                let key = (data.anchor.observation_id.clone(), data.anchor.frame_index);
                unavailable_by_anchor
                    .entry(key)
                    .and_modify(|state| {
                        if data.state == UnavailableState::Unknown {
                            *state = UnavailableState::Unknown;
                        }
                    })
                    .or_insert(data.state.clone());
                unavailable.insert(
                    record["id"].as_str().unwrap().to_owned(),
                    (record.clone(), data),
                );
            }
            None => {}
        }
    }
    let raw_anchor = raw_anchor.filter(|record| {
        (record["kind"] == "sample"
            && record["stack"]
                .as_array()
                .is_some_and(|stack| index < stack.len()))
            || (index == 0 && instances.contains_key(requested))
    });
    let frame = raw_anchor
        .and_then(|record| record["stack"].as_array())
        .and_then(|stack| stack.get(index));
    let mut artifacts = BTreeMap::new();
    let resolved = raw_anchor
        .filter(|record| record["kind"] == "sample" && frame.is_some())
        .map(|record| associations.resolve(record, index, &instances, port, &mut artifacts, work))
        .transpose()?;
    let initial = resolved
        .as_ref()
        .and_then(|resolved| resolved.id.clone())
        .or_else(|| {
            (raw_anchor.is_some() && index == 0 && instances.contains_key(requested))
                .then(|| requested.to_owned())
        });
    let point = resolved
        .as_ref()
        .and_then(|resolved| resolved.offset.as_deref())
        .map(code::hex)
        .transpose()?;
    let association_truncated = resolved
        .as_ref()
        .is_some_and(|resolved| resolved.ids.len() > limit);
    let anchor = raw_anchor.map(|record| {
        json!({
            "observation_id":requested,
            "kind":record["kind"],
            "frame_index":index,
            "timestamp_us":record["timestamp_us"],
            "aligned":placed(record),
            "frame":frame,
            "code_instance_id":initial,
            "code_offset":resolved.as_ref().and_then(|resolved|resolved.offset.as_deref()),
            "code_address_basis":resolved.as_ref().and_then(|resolved|resolved.basis.as_deref()),
            "association_status":resolved.as_ref().map(|resolved|resolved.status)
                .unwrap_or(if initial.is_some() {"inline"} else {"unavailable"}),
            "association_ids":resolved.as_ref()
                .map(|resolved|resolved.ids.iter().take(limit).collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });
    let anchor_time = raw_anchor
        .and_then(|record| record["timestamp_us"].as_u64())
        .unwrap_or(0);
    let anchor_attributes = raw_anchor
        .map(|record| &record["attributes"])
        .unwrap_or(&Value::Null);
    let mut outgoing: HashMap<String, Vec<String>> = HashMap::new();
    for (id, mapping) in &mappings {
        outgoing
            .entry(mapping.record["from"].as_str().unwrap_or("").to_owned())
            .or_default()
            .push(id.clone());
    }
    let mut selected_instances = BTreeSet::new();
    let mut selected_mappings = BTreeSet::new();
    let mut selected_frame_mappings = resolved
        .as_ref()
        .map(|resolved| resolved.ids.iter().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    for id in &selected_frame_mappings {
        let target = associations.rows[id].record["to"].as_str().unwrap_or("");
        if instances.contains_key(target) {
            selected_instances.insert(target.to_owned());
        }
    }
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    if let Some(id) = &initial {
        queue.push_back((id.clone(), None::<CodeRange>, point));
    }
    while let Some((id, range, point)) = queue.pop_front() {
        work.entry()?;
        let key = serde_json::to_string(&(&id, &range, point))?;
        if !visited.insert(key) {
            continue;
        }
        let Some(instance) = instances.get(&id) else {
            diagnostics.push(format!("Code instance {id} is not selected."));
            continue;
        };
        selected_instances.insert(id.clone());
        for mapping_id in outgoing.get(&id).into_iter().flatten() {
            work.frame()?;
            let mapping = &mappings[mapping_id];
            let intersects = if let Some(point) = point {
                mapping.data.from_range.contains(point)?
            } else if let Some(range) = &range {
                mapping.data.from_range.overlaps(range)?
            } else {
                true
            };
            if !intersects {
                continue;
            }
            if instance_at(&instance.data, anchor_time, anchor_attributes) == Some(false)
                || mapping
                    .data
                    .validity
                    .as_ref()
                    .and_then(|v| v.at_in(anchor_time, anchor_attributes))
                    == Some(false)
            {
                continue;
            }
            let target = mapping.record["to"].as_str().unwrap_or("");
            if let Some(target_instance) = instances.get(target) {
                if instance_at(&target_instance.data, anchor_time, anchor_attributes) == Some(false)
                {
                    continue;
                }
                selected_mappings.insert(mapping_id.clone());
                queue.push_back((target.to_owned(), Some(mapping.data.to_range.clone()), None));
            } else {
                diagnostics.push(format!(
                    "Mapping {mapping_id} has no selected target instance."
                ));
            }
        }
    }
    let mut instance_proofs = HashMap::new();
    let mut mapping_proofs = HashMap::new();
    let mut proof_instances = selected_instances.clone();
    let mut proof_mappings = selected_mappings.clone();
    if let Some(id) = &initial {
        proof_instances.insert(id.clone());
        for mapping_id in outgoing.get(id).into_iter().flatten() {
            proof_mappings.insert(mapping_id.clone());
            proof_instances.insert(
                mappings[mapping_id].record["to"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned(),
            );
        }
    }
    // Coverage uses all direct mappings, independently of returned display limits.
    for id in proof_instances {
        work.check()?;
        let Some(instance) = instances.get(&id) else {
            continue;
        };
        let valid = if let Some(reference) = &instance.data.artifact {
            proof_status(port, reference, &mut artifacts, work)?
        } else {
            true
        };
        instance_proofs.insert(id.clone(), valid);
    }
    for id in proof_mappings {
        work.check()?;
        let mapping = &mappings[&id];
        let mut valid = true;
        for reference in &mapping.data.proof.artifact_refs {
            valid &= proof_status(port, reference, &mut artifacts, work)?;
        }
        mapping_proofs.insert(id.clone(), valid);
    }
    let units = records
        .iter()
        .filter(|record| record["kind"] == "sample")
        .filter_map(|record| record["weight_unit"].as_str())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let unit = params["unit"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if units.contains("samples") {
                "samples".into()
            } else {
                units.first().cloned().unwrap_or_else(|| "samples".into())
            }
        });
    let measurable = raw_anchor.is_some_and(|record| record["kind"] == "sample")
        || initial
            .as_ref()
            .and_then(|id| instances.get(id))
            .is_some_and(|instance| instance.data.instance_kind == InstanceKind::Runtime)
        || initial.as_deref().is_some_and(|target| {
            associations
                .rows
                .values()
                .any(|mapping| mapping.record["to"] == target)
        });
    let available = measurable
        && records.iter().any(|record| {
            record["kind"] == "sample" && record["weight_unit"] == unit && placed(record)
        });
    let clock_domains = records
        .iter()
        .filter(|record| record["kind"] == "sample" && record["weight_unit"] == unit)
        .filter_map(|record| crate::model::clocks::runtime_relative_domain(&record["attributes"]))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let start = params["start_us"].as_u64();
    let end = params["end_us"].as_u64();
    let mut total = Bucket::default();
    let mut buckets: [Bucket; 4] = Default::default();
    let mut unaligned = 0u64;
    let mut scoped_ids = raw_anchor
        .into_iter()
        .map(|_| requested.to_owned())
        .collect::<HashSet<_>>();
    for record in &records {
        work.check()?;
        if record["kind"] != "sample" || record["weight_unit"] != unit {
            continue;
        }
        let mut matching = Vec::new();
        let mut positions = HashSet::new();
        for (frame_index, _) in record["stack"].as_array().into_iter().flatten().enumerate() {
            work.frame()?;
            if initial
                .as_deref()
                .is_some_and(|id| associations.may_reference(record, frame_index, id))
                || (initial.is_none() && record["id"] == requested && frame_index == index)
            {
                let resolved = associations.resolve(
                    record,
                    frame_index,
                    &instances,
                    port,
                    &mut artifacts,
                    work,
                )?;
                selected_frame_mappings.extend(resolved.ids.iter().cloned());
                if positions.insert((
                    resolved.id.clone(),
                    resolved.offset.clone(),
                    resolved.status,
                )) {
                    matching.push((frame_index, resolved));
                }
            }
        }
        if matching.is_empty() {
            continue;
        }
        if scoped_ids.insert(record["id"].as_str().unwrap().to_owned()) {
            work.entry()?;
        }
        let aligned = placed(record);
        if !aligned {
            unaligned += 1;
            continue;
        }
        let time = record["timestamp_us"].as_u64().unwrap_or(0);
        if start.is_some_and(|start| time < start) || end.is_some_and(|end| time > end) {
            continue;
        }
        let weight = record["weight"].as_f64().unwrap_or(0.0);
        total.add(weight);
        let mut states = vec![];
        let mut all_targets = HashSet::new();
        for (frame_index, resolved) in matching {
            let mut state = 3usize;
            let mut targets = HashSet::new();
            let mut shared = false;
            let mut uncertain = resolved.status == "ambiguous"
                || (resolved.status == "unavailable" && !resolved.ids.is_empty())
                || resolved.id.as_ref().is_some_and(|id| {
                    Some(id) != initial.as_ref()
                        || instance_proofs.get(id) != Some(&true)
                        || instances.get(id).and_then(|instance| {
                            instance_at(&instance.data, time, &record["attributes"])
                        }) != Some(true)
                });
            if aligned
                && let Some(id) = &initial
                && resolved.id.as_ref() == Some(id)
                && let Some(instance) = instances.get(id)
                && instance_at(&instance.data, time, &record["attributes"]) == Some(true)
                && instance_proofs.get(id) == Some(&true)
                && let Some(point) = resolved
                    .offset
                    .as_deref()
                    .and_then(|offset| code::hex(offset).ok())
            {
                for mapping_id in outgoing.get(id).into_iter().flatten() {
                    work.frame()?;
                    let mapping = &mappings[mapping_id];
                    if !mapping.data.from_range.contains(point)? {
                        continue;
                    }
                    let valid = mapping
                        .data
                        .validity
                        .as_ref()
                        .map(|v| v.at_in(time, &record["attributes"]))
                        .unwrap_or(Some(true));
                    if valid == Some(false) {
                        continue;
                    }
                    let target = mapping.record["to"].as_str().unwrap_or("");
                    let target_valid = instances
                        .get(target)
                        .and_then(|target| instance_at(&target.data, time, &record["attributes"]));
                    if valid.is_none()
                        || target_valid.is_none()
                        || mapping_proofs.get(mapping_id) != Some(&true)
                        || instance_proofs.get(target) != Some(&true)
                    {
                        uncertain = true;
                        continue;
                    }
                    if target_valid == Some(false) {
                        continue;
                    }
                    let target_key = serde_json::to_string(&(target, &mapping.data.to_range))?;
                    targets.insert(target_key.clone());
                    all_targets.insert(target_key);
                    shared |= mapping.data.cardinality == Cardinality::Shared;
                }
                state = if uncertain {
                    3
                } else if shared || targets.len() > 1 {
                    1
                } else if targets.len() == 1 {
                    0
                } else {
                    3
                };
            }
            if targets.is_empty()
                && !uncertain
                && let Some(unavailable) = unavailable_by_anchor
                    .get(&(record["id"].as_str().unwrap().to_owned(), frame_index))
            {
                state = if *unavailable == UnavailableState::Unmapped {
                    2
                } else {
                    3
                };
            }
            states.push(state);
        }
        let state = if states.contains(&3)
            || (states.contains(&2) && states.iter().any(|state| *state != 2))
        {
            3
        } else if states.contains(&1) || all_targets.len() > 1 {
            1
        } else {
            states[0]
        };
        buckets[state].add(weight);
    }
    for id in &selected_frame_mappings {
        let target = associations.rows[id].record["to"].as_str().unwrap_or("");
        if instances.contains_key(target) {
            selected_instances.insert(target.to_owned());
        }
    }
    let unavailable = unavailable
        .into_iter()
        .filter(|(_, (_, fact))| scoped_ids.contains(&fact.anchor.observation_id))
        .map(|(id, (_, unavailable))| json!({"id":id,"unavailable":unavailable}))
        .collect::<Vec<_>>();
    let mut has_origin = false;
    if let (Some(id), Some(point)) = (&initial, point) {
        for mapping_id in outgoing.get(id).into_iter().flatten() {
            work.frame()?;
            if selected_mappings.contains(mapping_id)
                && mappings[mapping_id].data.from_range.contains(point)?
            {
                has_origin = true;
                break;
            }
        }
    }
    let recording_diagnostics = if initial.is_some() && point.is_some() && !has_origin {
        diagnostics.push("No accepted code_maps_to range covers this captured byte coordinate. Retained code bytes alone do not establish a Wasm or source origin.".into());
        let run = port.run()?;
        let messages = run
            .diagnostics
            .iter()
            .filter(|message| !message.is_empty())
            .take(21)
            .collect::<Vec<_>>();
        let truncated = messages.len() > 20
            || messages
                .iter()
                .take(20)
                .any(|message| message.chars().take(2049).count() > 2048);
        Some(json!({
            "messages":messages.into_iter().take(20)
                .map(|message|message.chars().take(2048).collect::<String>())
                .collect::<Vec<_>>(),
            "truncated":truncated
        }))
    } else {
        None
    };
    let truncated = selected_instances.len() > limit
        || selected_mappings.len() > limit
        || selected_frame_mappings.len() > limit
        || association_truncated
        || unavailable.len() > limit
        || artifacts.len() > limit;
    let instances = selected_instances
        .into_iter()
        .take(limit)
        .map(|id| {
            let instance = &instances[&id];
            json!({
                "id":id,
                "basis":instance.record["basis"],
                "instance":instance.data,
                "evidence":instance.record.get("evidence").cloned().unwrap_or_else(||json!([]))
            })
        })
        .collect::<Vec<_>>();
    let mappings = selected_mappings
        .into_iter()
        .take(limit)
        .map(|id| {
            let mapping = &mappings[&id];
            json!({
                "id":id,"from":mapping.record["from"],"to":mapping.record["to"],
                "basis":mapping.record["basis"],"mapping":mapping.data,
                "evidence":mapping.record["evidence"]
            })
        })
        .collect::<Vec<_>>();
    let frame_mappings = selected_frame_mappings
        .into_iter()
        .take(limit)
        .map(|id| {
            let mapping = &associations.rows[&id];
            json!({
                "id":id,"from":mapping.record["from"],"to":mapping.record["to"],
                "basis":mapping.record["basis"],"frame_mapping":mapping.data,
                "evidence":mapping.record["evidence"]
            })
        })
        .collect::<Vec<_>>();
    if initial.is_none() {
        diagnostics.push("No unique applicable code association is available for this anchor; content names do not establish code identity.".into());
    }
    diagnostics.push("Coverage counts each selected raw sample once. Shared code is not apportioned to semantic units; proof verification fields are producer claims and metadata hashes do not reverify current artifact bytes.".into());
    let mut result = json!({
        "clock_domain":clock_domain,"clock_domains":clock_domains,
        "anchor":anchor,"instances":instances,"mappings":mappings,"frame_mappings":frame_mappings,
        "unavailable":unavailable.into_iter().take(limit).collect::<Vec<_>>(),
        "input_artifacts":artifacts.into_values().take(limit).collect::<Vec<_>>(),
        "units":units,
        "coverage":{
            "basis":"direct_code_mapping","unit":unit,
            "total":total.value(available),
            "mapped":buckets[0].value(available),
            "shared":buckets[1].value(available),
            "unmapped":buckets[2].value(available),
            "unknown":buckets[3].value(available),
            "unaligned_sample_count":unaligned,
            "status":if available {"complete"} else {"unavailable"}
        },
        "truncated":truncated,"diagnostics":diagnostics
    });
    if let Some(recording_diagnostics) = recording_diagnostics {
        result["recording_diagnostics"] = recording_diagnostics;
    }
    Ok(result)
}

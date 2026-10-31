//! Resolve selected mapper associations without changing retained sampled frames.
use super::*;
use crate::model::code::CodeFrameMapping;

pub(super) struct FrameMapping {
    pub record: Value,
    pub data: CodeFrameMapping,
}

#[derive(Default)]
pub(super) struct Associations {
    pub rows: BTreeMap<String, FrameMapping>,
    by_frame: HashMap<(String, usize), Vec<String>>,
}

pub(super) struct Association {
    pub id: Option<String>,
    pub offset: Option<String>,
    pub basis: Option<String>,
    pub status: &'static str,
    pub ids: Vec<String>,
}

impl Associations {
    pub fn insert(&mut self, record: &Value, data: CodeFrameMapping) {
        let id = record["id"].as_str().unwrap().to_owned();
        self.by_frame
            .entry((
                record["from"].as_str().unwrap().to_owned(),
                data.frame_index,
            ))
            .or_default()
            .push(id.clone());
        self.rows.insert(
            id,
            FrameMapping {
                record: record.clone(),
                data,
            },
        );
    }

    fn ids(&self, sample: &Value, index: usize) -> &[String] {
        self.by_frame
            .get(&(sample["id"].as_str().unwrap().to_owned(), index))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn may_reference(&self, sample: &Value, index: usize, target: &str) -> bool {
        sample["stack"][index]["code_instance_id"] == target
            || self
                .ids(sample, index)
                .iter()
                .any(|id| self.rows[id].record["to"] == target)
    }

    pub fn resolve(
        &self,
        sample: &Value,
        index: usize,
        instances: &BTreeMap<String, Instance>,
        port: &mut QueryReadPort<'_>,
        artifacts: &mut BTreeMap<(String, String), Value>,
        work: &mut WorkBudget,
    ) -> Result<Association> {
        let frame = &sample["stack"][index];
        let inline = frame["code_instance_id"].as_str();
        let mut resolved = Association {
            id: inline.map(str::to_owned),
            offset: frame["code_offset"].as_str().map(str::to_owned),
            basis: frame["code_address_basis"].as_str().map(str::to_owned),
            status: if inline.is_some() {
                "inline"
            } else {
                "unavailable"
            },
            ids: self.ids(sample, index).to_vec(),
        };
        let mut uncertain = false;
        let mut conflict = false;
        let time = sample["timestamp_us"].as_u64().unwrap_or(0);
        for id in &resolved.ids {
            work.frame()?;
            let mapping = &self.rows[id];
            let target = mapping.record["to"].as_str().unwrap_or("");
            let Some(instance) = instances.get(target) else {
                uncertain = true;
                continue;
            };
            let lifetime = instance_at(&instance.data, time, &sample["attributes"]);
            let validity = mapping
                .data
                .validity
                .as_ref()
                .map(|validity| validity.at_in(time, &sample["attributes"]))
                .unwrap_or(Some(true));
            if lifetime == Some(false) || validity == Some(false) {
                continue;
            }
            if lifetime.is_none()
                || validity.is_none()
                || (instance.data.instance_kind == InstanceKind::Runtime
                    && instance.record["pid"] != sample["pid"])
            {
                uncertain = true;
                continue;
            }
            let mut proven = true;
            if let Some(reference) = &instance.data.artifact {
                proven &= proof_status(port, reference, artifacts, work)?;
            }
            for reference in &mapping.data.proof.artifact_refs {
                proven &= proof_status(port, reference, artifacts, work)?;
            }
            if !proven {
                uncertain = true;
                continue;
            }
            if let Some(offset) = &mapping.data.code_offset
                && !instance
                    .data
                    .range
                    .as_ref()
                    .map(|range| range.contains(code::hex(offset)?))
                    .transpose()?
                    .unwrap_or(false)
            {
                uncertain = true;
                continue;
            }
            if resolved.id.as_deref().is_some_and(|id| id != target)
                || resolved
                    .offset
                    .as_ref()
                    .zip(mapping.data.code_offset.as_ref())
                    .is_some_and(|(a, b)| a != b)
            {
                conflict = true;
                continue;
            }
            resolved.id = Some(target.to_owned());
            if resolved.offset.is_none() {
                resolved.offset = mapping.data.code_offset.clone();
                resolved.basis = mapping.data.code_address_basis.clone();
            }
            if inline.is_none() {
                resolved.status = "derived";
            }
        }
        if conflict || uncertain {
            resolved.status = if conflict { "ambiguous" } else { "unavailable" };
            resolved.id = None;
            resolved.offset = None;
            resolved.basis = None;
        }
        Ok(resolved)
    }
}

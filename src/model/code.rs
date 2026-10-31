//! Typed evidence-backed code facts carried by ordinary observation records.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArtifactRef {
    pub artifact_id: String,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
}

impl ArtifactRef {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.artifact_id.is_empty(),
            "code artifact identity is empty"
        );
        ensure!(
            self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "code artifact digest must be lowercase SHA-256"
        );
        ensure!(
            self.offset.is_some() == self.length.is_some(),
            "code artifact offset and length must be supplied together"
        );
        if let (Some(offset), Some(length)) = (self.offset, self.length) {
            ensure!(
                length > 0 && offset.checked_add(length).is_some(),
                "code artifact byte range is empty or overflows"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RangeSpace {
    RuntimeAddress,
    ArtifactOffset,
    FunctionOffset,
}

pub(crate) fn hex(value: &str) -> Result<u64> {
    let digits = value
        .strip_prefix("0x")
        .context("code coordinate needs 0x prefix")?;
    ensure!(
        !digits.is_empty()
            && (digits.len() == 1 || !digits.starts_with('0'))
            && digits
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "code coordinate must be canonical lowercase hexadecimal"
    );
    u64::from_str_radix(digits, 16).context("code coordinate exceeds 64 bits")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodeRange {
    pub space: RangeSpace,
    pub start: String,
    pub end_exclusive: String,
    pub function_index: Option<u64>,
}
impl CodeRange {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            hex(&self.start)? < hex(&self.end_exclusive)?,
            "code range must be nonempty and half-open"
        );
        ensure!(
            (self.space == RangeSpace::FunctionOffset) == self.function_index.is_some(),
            "function index is required only for function offsets"
        );
        Ok(())
    }
    pub fn contains(&self, point: u64) -> Result<bool> {
        Ok(hex(&self.start)? <= point && point < hex(&self.end_exclusive)?)
    }
    pub fn overlaps(&self, other: &Self) -> Result<bool> {
        Ok(self.space == other.space
            && self.function_index == other.function_index
            && hex(&self.start)? < hex(&other.end_exclusive)?
            && hex(&other.start)? < hex(&self.end_exclusive)?)
    }
    pub fn contains_range(&self, other: &Self) -> Result<bool> {
        Ok(self.space == other.space
            && self.function_index == other.function_index
            && hex(&self.start)? <= hex(&other.start)?
            && hex(&other.end_exclusive)? <= hex(&self.end_exclusive)?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodeValidity {
    pub clock: String,
    pub alignment: String,
    pub start_us: Option<u64>,
    pub end_us: Option<u64>,
    pub start_censored: bool,
    pub end_censored: bool,
    pub boundary_basis: String,
}
impl CodeValidity {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.clock.is_empty() && !self.alignment.is_empty() && !self.boundary_basis.is_empty(),
            "code validity provenance is required"
        );
        for value in [self.start_us, self.end_us].into_iter().flatten() {
            ensure!(
                value <= 9_007_199_254_740_991,
                "code time exceeds exact protocol range"
            );
        }
        if let (Some(start), Some(end)) = (self.start_us, self.end_us) {
            ensure!(start < end, "code lifetime must be nonempty and half-open");
        }
        super::clocks::validate(
            &serde_json::json!({"clock":self.clock,"clock_alignment":self.alignment}),
        )
    }
    pub fn at(&self, timestamp: u64, aligned: bool) -> Option<bool> {
        let attrs = serde_json::json!({"clock":self.clock,"clock_alignment":self.alignment});
        if !aligned || !super::clocks::inspect(&attrs).aligned {
            return None;
        }
        self.within(timestamp)
    }

    pub fn at_in(&self, timestamp: u64, attributes: &Value) -> Option<bool> {
        if self.alignment == "runtime_relative"
            && super::clocks::runtime_relative_domain(attributes) == Some(self.clock.as_str())
        {
            let declared = serde_json::json!({"clock":self.clock,"clock_alignment":self.alignment});
            super::clocks::runtime_relative_domain(&declared)?;
            return self.within(timestamp);
        }
        self.at(timestamp, super::clocks::inspect(attributes).aligned)
    }

    fn within(&self, timestamp: u64) -> Option<bool> {
        if self.start_us.is_some_and(|start| timestamp < start) {
            return (!self.start_censored).then_some(false);
        }
        if self.end_us.is_some_and(|end| timestamp >= end) {
            return (!self.end_censored).then_some(false);
        }
        (self.start_us.is_some() && self.end_us.is_some()).then_some(true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InstanceKind {
    Runtime,
    Static,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Representation {
    MachineCode,
    Wasm,
    Bytecode,
    Source,
    Logical,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodeSource {
    pub path: String,
    pub line: Option<u64>,
    pub column: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodeInstance {
    pub schema_version: u32,
    pub instance_kind: InstanceKind,
    pub runtime_instance_id: Option<String>,
    pub representation: Representation,
    pub architecture: Option<String>,
    pub code_id: Option<String>,
    pub artifact: Option<ArtifactRef>,
    pub range: Option<CodeRange>,
    pub validity: Option<CodeValidity>,
    pub label: Option<String>,
    pub source: Option<CodeSource>,
}
impl CodeInstance {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 1, "unsupported code instance schema");
        match self.instance_kind {
            InstanceKind::Runtime => ensure!(
                self.runtime_instance_id
                    .as_ref()
                    .is_some_and(|id| !id.is_empty())
                    && self.validity.is_some(),
                "runtime code needs runtime identity and explicit validity"
            ),
            InstanceKind::Static => ensure!(
                self.runtime_instance_id.is_none() && self.validity.is_none(),
                "static code units cannot declare a runtime lifetime"
            ),
        }
        if let Some(artifact) = &self.artifact {
            artifact.validate()?;
        }
        if let Some(range) = &self.range {
            range.validate()?;
            ensure!(
                range.space != RangeSpace::RuntimeAddress
                    || self.instance_kind == InstanceKind::Runtime,
                "static code cannot own a runtime address"
            );
        }
        if let Some(validity) = &self.validity {
            validity.validate()?;
        }
        if let Some(source) = &self.source {
            ensure!(
                !source.path.is_empty()
                    && source
                        .line
                        .is_none_or(|line| line > 0 && line <= 9_007_199_254_740_991)
                    && source
                        .column
                        .is_none_or(|column| column <= 9_007_199_254_740_991),
                "invalid typed code source location"
            );
        }
        ensure!(
            self.representation != Representation::Source
                || (self.source.is_some() && self.artifact.is_some()),
            "source code units need a typed source location and retained artifact identity"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Cardinality {
    Exclusive,
    Shared,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Verification {
    Declared,
    IdentityChecked,
    SemanticChecked,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingProof {
    pub artifact_refs: Vec<ArtifactRef>,
    pub method: String,
    pub verification: Verification,
    pub validator: Option<String>,
    #[serde(default)]
    pub semantic_evidence: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodeMapping {
    pub schema_version: u32,
    pub from_range: CodeRange,
    pub to_range: CodeRange,
    pub cardinality: Cardinality,
    pub validity: Option<CodeValidity>,
    pub proof: MappingProof,
}
impl CodeMapping {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 1, "unsupported code mapping schema");
        self.from_range.validate()?;
        self.to_range.validate()?;
        if let Some(validity) = &self.validity {
            validity.validate()?;
        }
        ensure!(
            !self.proof.method.is_empty(),
            "code mapping proof method is empty"
        );
        for reference in &self.proof.artifact_refs {
            reference.validate()?;
        }
        if self.proof.verification == Verification::SemanticChecked {
            ensure!(
                self.proof.validator.as_ref().is_some_and(|v| !v.is_empty())
                    && !self.proof.semantic_evidence.is_empty(),
                "semantic mapping claims need named validator evidence"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodeAnchor {
    pub observation_id: String,
    pub frame_index: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UnavailableState {
    Unmapped,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodeUnavailable {
    pub schema_version: u32,
    pub anchor: CodeAnchor,
    pub state: UnavailableState,
    pub reason_code: String,
}

#[derive(Debug, Clone)]
pub(crate) enum CodeRecord {
    Instance(CodeInstance),
    Mapping(CodeMapping),
    Unavailable(CodeUnavailable),
}
impl CodeRecord {
    pub fn artifact_refs(&self) -> Vec<&ArtifactRef> {
        match self {
            Self::Instance(instance) => instance.artifact.iter().collect(),
            Self::Mapping(mapping) => mapping.proof.artifact_refs.iter().collect(),
            Self::Unavailable(_) => vec![],
        }
    }
}

pub(crate) fn validate_frame(frame: &Value) -> Result<()> {
    if let Some(basis) = frame.get("code_address_basis") {
        ensure!(
            basis.as_str().is_some_and(|basis| !basis.trim().is_empty()),
            "code address basis must be a nonempty string"
        );
    }
    if let Some(id) = frame.get("code_instance_id") {
        ensure!(
            id.as_str().is_some_and(|id| !id.is_empty()),
            "frame code instance identity is empty"
        );
    }
    if let Some(offset) = frame.get("code_offset") {
        hex(offset
            .as_str()
            .context("frame code offset must be a string")?)?;
        ensure!(
            frame["code_instance_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "frame code offset requires a code instance identity"
        );
    }
    Ok(())
}

pub(crate) fn parse_record(record: &Value) -> Result<Option<CodeRecord>> {
    for frame in record["stack"].as_array().into_iter().flatten() {
        validate_frame(frame)?;
    }
    let attributes = &record["attributes"];
    match (record["name"].as_str(), record["relation"].as_str()) {
        (Some("code.instance"), _) => {
            ensure!(
                record["kind"] == "event",
                "code instance declaration must be an event"
            );
            let instance: CodeInstance =
                serde_json::from_value(attributes["code_instance"].clone())
                    .context("invalid code instance")?;
            instance.validate()?;
            Ok(Some(CodeRecord::Instance(instance)))
        }
        (_, Some("code_maps_to")) => {
            ensure!(
                record["kind"] == "relation",
                "code mapping must be a relation"
            );
            let mapping: CodeMapping = serde_json::from_value(attributes["code_mapping"].clone())
                .context("invalid code mapping")?;
            mapping.validate()?;
            Ok(Some(CodeRecord::Mapping(mapping)))
        }
        (Some("code.mapping_unavailable"), _) => {
            ensure!(
                record["kind"] == "event",
                "code mapping availability must be an event"
            );
            let unavailable: CodeUnavailable =
                serde_json::from_value(attributes["code_mapping_unavailable"].clone())
                    .context("invalid code availability")?;
            ensure!(
                unavailable.schema_version == 1
                    && !unavailable.anchor.observation_id.is_empty()
                    && !unavailable.reason_code.is_empty(),
                "invalid code availability declaration"
            );
            Ok(Some(CodeRecord::Unavailable(unavailable)))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn instance() -> Value {
        json!({"kind":"event","name":"code.instance","attributes":{"code_instance":{"schema_version":1,"instance_kind":"static","representation":"wasm","artifact":{"artifact_id":"artifact","sha256":"a".repeat(64)},"range":{"space":"function_offset","start":"0x0","end_exclusive":"0x20","function_index":7}}}})
    }

    #[test]
    fn static_units_are_not_runtime_instances_and_recognized_fields_are_strict() {
        assert!(parse_record(&instance()).unwrap().is_some());
        let mut malformed = instance();
        malformed["attributes"]["code_instance"]["instance_kind"] = json!("runtime");
        assert!(parse_record(&malformed).is_err());
        malformed = instance();
        malformed["attributes"]["code_instance"]["pretend_verified"] = json!(true);
        assert!(parse_record(&malformed).is_err());
        assert!(
            parse_record(
                &json!({"kind":"event","name":"legacy","attributes":{"opaque":{"arbitrary":true}}})
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn ranges_and_artifact_slices_cannot_hide_ambiguous_or_overflowing_coordinates() {
        for range in [
            json!({"space":"artifact_offset","start":"0x01","end_exclusive":"0x20"}),
            json!({"space":"artifact_offset","start":"0x20","end_exclusive":"0x20"}),
            json!({"space":"function_offset","start":"0x0","end_exclusive":"0x20"}),
        ] {
            let mut malformed = instance();
            malformed["attributes"]["code_instance"]["range"] = range;
            assert!(parse_record(&malformed).is_err());
        }
        for artifact in [
            json!({"artifact_id":"artifact","sha256":"a".repeat(64),"offset":1}),
            json!({"artifact_id":"artifact","sha256":"a".repeat(64),"offset":u64::MAX,"length":2}),
        ] {
            let mut malformed = instance();
            malformed["attributes"]["code_instance"]["artifact"] = artifact;
            assert!(parse_record(&malformed).is_err());
        }
    }

    #[test]
    fn lifetime_boundaries_and_unknown_clocks_are_not_invented() {
        let mut validity = CodeValidity {
            clock: "unix_epoch".into(),
            alignment: "direct_epoch".into(),
            start_us: Some(10),
            end_us: Some(20),
            start_censored: false,
            end_censored: false,
            boundary_basis: "runtime_events".into(),
        };
        assert_eq!(validity.at(10, true), Some(true));
        assert_eq!(validity.at(19, true), Some(true));
        assert_eq!(validity.at(20, true), Some(false));
        assert_eq!(validity.at(15, false), None);
        validity.end_censored = true;
        assert_eq!(validity.at(20, true), None);
        validity.start_censored = true;
        assert_eq!(validity.at(9, true), None);
        validity.clock = "thread_cpu".into();
        validity.alignment = "unknown".into();
        assert_eq!(validity.at(15, true), None);
        validity.clock = "unix_epoch".into();
        validity.alignment = "direct_epoch".into();
        validity.start_us = None;
        validity.end_censored = false;
        assert_eq!(validity.at(20, true), Some(false));
        assert_eq!(validity.at(15, true), None);
        assert!(validate_frame(&json!({"function":"f","code_offset":"0x1"})).is_err());
        assert!(validate_frame(&json!({"function":"f","address":"0x1001","code_instance_id":"i","code_offset":"0x1"})).is_ok());
    }

    #[test]
    fn explicit_runtime_clock_domains_compare_only_with_the_same_known_origin() {
        let validity = CodeValidity {
            clock: "v8:pid7:profile-a".into(),
            alignment: "runtime_relative".into(),
            start_us: Some(10),
            end_us: Some(20),
            start_censored: false,
            end_censored: false,
            boundary_basis: "v8_code_tick_events".into(),
        };
        let same = json!({"clock":"v8:pid7:profile-a","clock_alignment":"runtime_relative"});
        assert_eq!(validity.at_in(10, &same), Some(true));
        assert_eq!(validity.at_in(20, &same), Some(false));
        assert_eq!(
            validity.at_in(
                15,
                &json!({"clock":"v8:pid7:profile-b","clock_alignment":"runtime_relative"})
            ),
            None
        );
        assert_eq!(
            validity.at_in(
                15,
                &json!({"clock":"v8:pid7:profile-a","clock_alignment":"unknown"})
            ),
            None
        );
        assert_eq!(
            validity.at_in(
                15,
                &json!({"clock":"unix_epoch","clock_alignment":"direct_epoch"})
            ),
            None
        );
    }
}

use super::{Frame, Profile};
use crate::process::{Error, ErrorKind, Result};
use hunin_core::{
    project::TargetRef,
    records::{Clock, FieldSchema, FieldType, Record, RecordSchema, RecordTime, SourceDefinition},
};
use std::collections::BTreeMap;

fn schema(name: &str, fields: &[(&str, FieldType, &str)]) -> RecordSchema {
    RecordSchema {
        name: name.into(),
        version: 1,
        description: format!("{name}; source-specific evidence, not inferred operation ownership"),
        fields: fields
            .iter()
            .map(|(name, kind, meaning)| {
                (
                    name.to_string(),
                    FieldSchema {
                        kind: kind.clone(),
                        required: true,
                        unit: None,
                        meaning: meaning.to_string(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>(),
    }
}
impl Profile {
    pub fn definition(&self, id: &str, target: TargetRef) -> SourceDefinition {
        let mut metadata = self.metadata.clone();
        if let Some(object) = metadata.as_object_mut() {
            object.remove("lifetime_events");
        }
        SourceDefinition{id:id.into(),target:Some(target),interpretation:format!("hunin-platforms/{}/1",self.schema),metadata:serde_json::json!({"producer":self.producer,"language":self.language,"format":self.schema,"sample_clock":self.clock,"sample_weight":"per-record source unit; not exact function duration","aggregate_count":"inclusive producer count; ancestors and descendants must not be summed","issues":self.issues,"source_metadata":metadata,"period":self.period,"start":self.start,"end":self.end}),clocks:self.clock.iter().map(|id|Clock{id:id.clone(),ticks_per_second:self.ticks_per_second,description:"source sample clock in declared ticks per second; no automatic relation to other clocks".into()}).collect(),schemas:vec![
   schema("native.capture",&[("producer",FieldType::String,"actual recording producer"),("format",FieldType::String,"source format and decoder interpretation"),("issues",FieldType::Array,"loss, unsupported content and interpretation limits discovered during recording/extraction"),("metadata",FieldType::Object,"final source-specific recording configuration and interpretation; lifecycle events have their own rows")]),
   schema("native.event",&[("provider",FieldType::String,"source lifetime/event provider"),("timestamp",FieldType::Nullable(Box::new(FieldType::Unsigned)),"original source clock timestamp when present; null means the ABI omitted event time"),("properties",FieldType::Object,"source-decoded event properties with explicit unsupported-field entries")]),
   schema("native.image",&[("image",FieldType::Object,"loaded image mapping, architecture, identity and relocation")]),
   schema("native.sample",&[("producer_fields",FieldType::Object,"source payload fields, provider version and pointer layout without invented semantics"),("instruction_pointer",FieldType::Unsigned,"sampled instruction pointer retained independently of callchain"),("process_id",FieldType::Unsigned,"sampled process id"),("thread_id",FieldType::Unsigned,"sampled thread id"),("timestamp",FieldType::Unsigned,"exact source clock timestamp"),("weight",FieldType::Unsigned,"source sample weight"),("weight_unit",FieldType::String,"source weight definition"),("frame_count",FieldType::Unsigned,"number of ordered frame records")]),
   schema("native.inline",&[("physical_record",FieldType::String,"source-local frame/aggregate record being expanded"),("inline_position",FieldType::Unsigned,"symbolizer inline context order after the physical record's first source site"),("site",FieldType::Object,"build-matched inline function and source context; not an additional sample or invocation"),("address",FieldType::Unsigned,"same recorded physical instruction pointer")]),
   schema("native.frame",&[("sample",FieldType::String,"source-local sample record id"),("position",FieldType::Unsigned,"callchain order as emitted, including context markers"),("frame",FieldType::Object,"address, matched build, source and unresolved reason")]),
   schema("native.aggregate",&[("node",FieldType::Unsigned,"producer tree node id"),("count",FieldType::Unsigned,"inclusive producer count, not nanoseconds"),("frame",FieldType::Object,"producer frame with source interpretation")]),
  ]}
    }
    pub fn records(&self, subject: &str) -> Result<Vec<Record>> {
        if self.samples.len() > 1_000_000
            || self.aggregates.len() > 1_000_000
            || self.images.len() > 100_000
        {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "native record count exceeds import budget",
            ));
        }
        let mut result = Vec::new();
        let mut bytes = 0usize;
        let mut push = |record: Record| -> Result<()> {
            bytes = bytes
                .checked_add(
                    serde_json::to_vec(&record)
                        .map_err(|e| Error::invalid(e.to_string()))?
                        .len(),
                )
                .ok_or_else(|| Error::invalid("native extraction size overflow"))?;
            if bytes > 64 * 1024 * 1024 {
                return Err(Error::new(
                    ErrorKind::LimitExceeded,
                    "native structured extraction exceeds 64 mib",
                ));
            }
            result.push(record);
            Ok(())
        };
        if let Some(events) = self
            .metadata
            .get("lifetime_events")
            .and_then(|v| v.as_array())
        {
            for (i, event) in events.iter().enumerate() {
                push(record(
                    format!("event-{i}"),
                    "native.event",
                    subject,
                    None,
                    None,
                    event.clone(),
                ))?;
            }
        }
        let mut metadata = self.metadata.clone();
        if let Some(object) = metadata.as_object_mut() {
            object.remove("lifetime_events");
        }
        push(record(
            "capture".into(),
            "native.capture",
            subject,
            None,
            None,
            serde_json::json!({"producer":self.producer,"format":self.schema,"declared_build_language":self.language,"clock":self.clock,"ticks_per_second":self.clock.as_ref().map(|_|self.ticks_per_second),"period":self.period,"start":self.start,"end":self.end,"issues":self.issues,"metadata":metadata}),
        ))?;
        for (i, image) in self.images.iter().enumerate() {
            push(record(
                format!("image-{i}"),
                "native.image",
                subject,
                None,
                None,
                serde_json::json!({"image":image}),
            ))?;
        }
        for (i, sample) in self.samples.iter().enumerate() {
            if sample.frames.len() > 1024 {
                return Err(Error::new(
                    ErrorKind::LimitExceeded,
                    "native sample exceeds frame budget",
                ));
            }
            let time = self
                .clock
                .as_ref()
                .map(|clock| {
                    i64::try_from(sample.timestamp).map(|start| RecordTime {
                        clock: clock.clone(),
                        start,
                        end: None,
                        uncertainty: 0,
                    })
                })
                .transpose()
                .map_err(|_| Error::invalid("sample clock exceeds signed record range"))?;
            let id = format!("sample-{i}");
            push(record(
                id.clone(),
                "native.sample",
                subject,
                time.clone(),
                None,
                serde_json::json!({"process_id":sample.process_id,"thread_id":sample.thread_id,"timestamp":sample.timestamp,"cpu":sample.cpu,"weight":sample.weight,"weight_unit":sample.weight_unit,"frame_count":sample.frames.len(),"instruction_pointer":sample.instruction_pointer,"producer_fields":sample.producer_fields}),
            ))?;
            for (position, frame) in sample.frames.iter().enumerate() {
                push(record(
                    format!("frame-{i}-{position}"),
                    "native.frame",
                    subject,
                    time.clone(),
                    Some(frame),
                    serde_json::json!({"sample":id,"position":position,"frame":frame}),
                ))?;
                for (inline, site) in frame.inline_sites.iter().enumerate() {
                    push(inline_record(
                        format!("frame-{i}-{position}-inline-{inline}"),
                        &format!("frame-{i}-{position}"),
                        inline,
                        site,
                        frame.address,
                        subject,
                        time.clone(),
                    ))?;
                }
            }
        }
        for node in &self.aggregates {
            push(record(
                format!("aggregate-{}", node.id),
                "native.aggregate",
                subject,
                None,
                Some(&node.frame),
                serde_json::json!({"node":node.id,"parent":node.parent,"thread_id":node.thread_id,"count":node.count,"frame":node.frame}),
            ))?;
            for (inline, site) in node.frame.inline_sites.iter().enumerate() {
                push(inline_record(
                    format!("aggregate-{}-inline-{inline}", node.id),
                    &format!("aggregate-{}", node.id),
                    inline,
                    site,
                    node.frame.address,
                    subject,
                    None,
                ))?;
            }
        }
        Ok(result)
    }
}
fn record(
    id: String,
    schema: &str,
    subject: &str,
    time: Option<RecordTime>,
    frame: Option<&Frame>,
    payload: serde_json::Value,
) -> Record {
    Record {
        id,
        schema: schema.into(),
        schema_version: 1,
        subject: subject.into(),
        time,
        function: frame.and_then(|f| f.function.clone()),
        location: frame
            .and_then(|f| f.source.as_ref())
            .map(|s| format!("{}:{}", s.file, s.line)),
        operation: None,
        payload,
    }
}

fn inline_record(
    id: String,
    physical: &str,
    position: usize,
    site: &super::InlineSite,
    address: u64,
    subject: &str,
    time: Option<RecordTime>,
) -> Record {
    Record {
        id,
        schema: "native.inline".into(),
        schema_version: 1,
        subject: subject.into(),
        time,
        function: site.function.clone(),
        location: site
            .source
            .as_ref()
            .map(|source| format!("{}:{}", source.file, source.line)),
        operation: None,
        payload: serde_json::json!({"physical_record":physical,"inline_position":position+1,"site":site,"address":address,"adds_sample_weight":false}),
    }
}

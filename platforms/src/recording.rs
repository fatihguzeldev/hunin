use crate::process::{Error, ResourceSnapshot, Result};
use hunin_core::{
    project::TargetRef,
    records::{Clock, FieldSchema, FieldType, Record, RecordSchema, RecordTime, SourceDefinition},
};
use std::collections::BTreeMap;

pub fn resource_definition(
    id: &str,
    target: TargetRef,
    first: &ResourceSnapshot,
) -> SourceDefinition {
    let fields: BTreeMap<_, _> = [
        ("instance", FieldType::Object, "actual process instance; pid reuse is a different instance"),
        ("acquired", FieldType::Object, "monotonic interval bracketing resource acquisition; fields need not change at the same instant"),
        ("user_cpu", FieldType::Object, "cumulative process user cpu, with exact raw units and rational nanosecond scale; excludes children"),
        ("system_cpu", FieldType::Object, "cumulative process system cpu, with exact raw units and rational nanosecond scale; excludes children"),
        ("memory", FieldType::Array, "named memory measurements with bytes and source approximation flag; different definitions are not interchangeable"),
    ].into_iter().map(|(name, kind, meaning)| (name.into(), FieldSchema { kind, required: true, unit: None, meaning: meaning.into() })).collect();
    SourceDefinition {
        id: id.into(), target: Some(target),
        schemas: vec![RecordSchema { name: "platform.process_resources".into(), version: 1, fields, description: "process resource counters with acquisition bounds, instance identity, and source-specific definitions".into() }],
        clocks: vec![Clock { id: first.acquired.clock.clone(), ticks_per_second: 1_000_000_000, description: "host monotonic acquisition clock; record time is the read interval, not operation ownership".into() }],
        interpretation: "hunin-platforms/process-resources/1".into(),
        metadata: serde_json::json!({"platform":first.instance.platform,"instance":first.instance,"memory_definitions":first.memory.iter().map(|m|m.definition.as_str()).collect::<Vec<_>>(),"cpu_scope":"process_all_threads_excluding_children","time_meaning":"acquisition_interval"}),
    }
}

impl ResourceSnapshot {
    pub fn to_record(&self, id: &str, subject: &str) -> Result<Record> {
        self.user_cpu.nanoseconds()?;
        self.system_cpu.nanoseconds()?;
        if self.user_cpu.source_unit.is_empty()
            || self.system_cpu.source_unit.is_empty()
            || self.acquired.clock.is_empty()
            || self.acquired.end_ns < self.acquired.start_ns
        {
            return Err(Error::invalid(
                "resource units, clock, and acquisition bounds must be valid before publication",
            ));
        }

        let start = i64::try_from(self.acquired.start_ns)
            .map_err(|_| Error::invalid("acquisition clock exceeds record timestamp range"))?;
        let end = i64::try_from(self.acquired.end_ns)
            .map_err(|_| Error::invalid("acquisition clock exceeds record timestamp range"))?;
        Ok(Record {
            id: id.into(),
            schema: "platform.process_resources".into(),
            schema_version: 1,
            subject: subject.into(),
            time: Some(RecordTime {
                clock: self.acquired.clock.clone(),
                start,
                end: Some(end),
                uncertainty: 0,
            }),
            function: None,
            location: None,
            operation: None,
            payload: serde_json::to_value(self).map_err(|e| Error::invalid(e.to_string()))?,
        })
    }
}

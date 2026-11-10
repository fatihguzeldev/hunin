use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::project::{Error, Project, Reference, Result, TargetRef};

#[cfg(test)]
mod tests;

pub const MAX_RECORD_BYTES: usize = crate::project::MAX_VALUE_BYTES;
pub const MAX_BATCH_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BATCH_RECORDS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldType {
    Boolean,
    Signed,
    Unsigned,
    Float,
    String,
    Object,
    Array,
    Nullable(Box<FieldType>),
}

impl FieldType {
    pub(crate) fn accepts(&self, value: &Value) -> bool {
        match self {
            Self::Boolean => value.is_boolean(),
            Self::Signed => value.as_i64().is_some(),
            Self::Unsigned => value.as_u64().is_some(),
            Self::Float => value.as_f64().is_some_and(f64::is_finite),
            Self::String => value.is_string(),
            Self::Object => value.is_object(),
            Self::Array => value.is_array(),
            Self::Nullable(inner) => value.is_null() || inner.accepts(value),
        }
    }

    pub fn supports_equality(&self) -> bool {
        match self {
            Self::Object | Self::Array => false,
            Self::Nullable(inner) => inner.supports_equality(),
            _ => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldSchema {
    pub kind: FieldType,
    pub required: bool,
    pub unit: Option<String>,
    pub meaning: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordSchema {
    pub name: String,
    pub version: u32,
    pub fields: BTreeMap<String, FieldSchema>,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clock {
    pub id: String,
    pub ticks_per_second: u64,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordTime {
    pub clock: String,
    pub start: i64,
    pub end: Option<i64>,
    pub uncertainty: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub schema: String,
    pub schema_version: u32,
    pub subject: String,
    pub time: Option<RecordTime>,
    pub function: Option<String>,
    pub location: Option<String>,
    pub operation: Option<String>,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceDefinition {
    pub id: String,
    pub target: Option<TargetRef>,
    pub schemas: Vec<RecordSchema>,
    pub clocks: Vec<Clock>,
    pub interpretation: String,
    pub metadata: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IngestOutcome {
    pub inserted: usize,
    pub replayed: usize,
    pub revision: u64,
}

#[derive(Clone, Debug)]
pub struct SourceHandle {
    pub(crate) project: Project,
    pub(crate) run: String,
    pub(crate) id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceStatus {
    pub definition: SourceDefinition,
    pub ready: bool,
    pub closed: bool,
    pub cutoff: Option<u64>,
    pub outcome: Option<Value>,
    pub gap_count: u64,
}

impl SourceHandle {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn run(&self) -> Reference {
        Reference {
            project: self.project.id().to_owned(),
            id: self.run.clone(),
        }
    }

    pub fn definition(&self) -> Result<SourceDefinition> {
        self.project.with_connection(|connection| {
            let body: String = connection
                .query_row(
                    "SELECT definition FROM sources WHERE run=?1 AND id=?2",
                    params![self.run, self.id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| Error::NotFound(format!("source {}", self.id)))?;
            Ok(serde_json::from_str(&body)?)
        })
    }

    pub fn status(&self) -> Result<SourceStatus> {
        self.project.with_connection(|connection| {
            let (definition,closed,cutoff,outcome,ready,gap_count): (String,bool,Option<u64>,Option<String>,bool,u64) = connection.query_row("SELECT definition,closed,cutoff,outcome,EXISTS(SELECT 1 FROM source_events WHERE run=?1 AND source=?2 AND kind='ready'),(SELECT count(*) FROM source_events WHERE run=?1 AND source=?2 AND kind='gap') FROM sources WHERE run=?1 AND id=?2",params![self.run,self.id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional()?.ok_or_else(||Error::NotFound(format!("source {}",self.id)))?;
            Ok(SourceStatus { definition:serde_json::from_str(&definition)?,ready,closed,cutoff,outcome:outcome.map(|body|serde_json::from_str(&body)).transpose()?,gap_count })
        })
    }

    pub fn ready(&self, metadata: Value) -> Result<u64> {
        self.event("ready", "ready", metadata)
    }

    pub fn report_gap(&self, id: &str, gap: Value) -> Result<u64> {
        crate::project::validate_id(id)?;
        self.event("gap", id, gap)
    }

    fn event(&self, kind: &str, id: &str, value: Value) -> Result<u64> {
        let body = crate::project::encode_bounded(&value)?;
        self.project.transaction(|transaction| {
            let closed: bool=transaction.query_row("SELECT closed FROM sources WHERE run=?1 AND id=?2",params![self.run,self.id],|row|row.get(0))?;
            if closed { return Err(Error::Closed); }
            let existing: Option<(String,u64)>=transaction.query_row("SELECT body,revision FROM source_events WHERE run=?1 AND source=?2 AND kind=?3 AND id=?4",params![self.run,self.id,kind,id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
            if let Some((old,revision))=existing {
                return if old==body { Ok(revision) } else { Err(Error::Conflict(format!("source event {id}"))) };
            }
            crate::project::ensure_run_active(transaction,&self.run)?;
            let revision=crate::project::bump_revision(transaction)?;
            transaction.execute("INSERT INTO source_events(run,source,kind,id,body,revision) VALUES (?1,?2,?3,?4,?5,?6)",params![self.run,self.id,kind,id,body,revision])?;
            Ok(revision)
        })
    }

    pub fn append(&self, records: &[Record]) -> Result<IngestOutcome> {
        self.publish(None, records)
    }

    pub fn append_batch(&self, delivery_id: &str, records: &[Record]) -> Result<IngestOutcome> {
        crate::project::validate_id(delivery_id)?;
        self.publish(Some(delivery_id), records)
    }

    fn publish(&self, delivery: Option<&str>, records: &[Record]) -> Result<IngestOutcome> {
        if records.len() > MAX_BATCH_RECORDS {
            return Err(Error::Limit("batch exceeds 256 records".into()));
        }
        let definition = self.definition()?;
        let mut encoded = Vec::with_capacity(records.len());
        let mut total = 0_usize;
        let mut digest = Sha256::new();
        for record in records {
            validate_record(record, &definition)?;
            let body = crate::project::encode_bounded(record)?;
            total = total
                .checked_add(body.len())
                .ok_or_else(|| Error::Limit("batch size overflow".into()))?;
            if total > MAX_BATCH_BYTES {
                return Err(Error::Limit("batch exceeds 4 mib".into()));
            }
            digest.update((body.len() as u64).to_le_bytes());
            digest.update(body.as_bytes());
            encoded.push(body);
        }
        let digest = digest.finalize().to_vec();
        self.project.transaction(|transaction| {
            if let Some(delivery)=delivery {
                let existing:Option<(Vec<u8>,u64)>=transaction.query_row("SELECT digest,revision FROM delivery_batches WHERE run=?1 AND source=?2 AND id=?3",params![self.run,self.id,delivery],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
                if let Some((old,revision))=existing {
                    return if old==digest { Ok(IngestOutcome{inserted:0,replayed:records.len(),revision}) } else { Err(Error::Conflict(format!("delivery {delivery}"))) };
                }
            }
            let closed:bool=transaction.query_row("SELECT closed FROM sources WHERE run=?1 AND id=?2",params![self.run,self.id],|row|row.get(0))?;
            if delivery.is_some() {
                if closed {return Err(Error::Closed);}
                crate::project::ensure_run_active(transaction,&self.run)?;
            }
            let mut outcome=IngestOutcome{inserted:0,replayed:0,revision:crate::project::revision(transaction)?};
            for (record,body) in records.iter().zip(&encoded) {
                let old:Option<String>=transaction.query_row("SELECT body FROM records WHERE run=?1 AND source=?2 AND id=?3",params![self.run,self.id,record.id],|row|row.get(0)).optional()?;
                if let Some(old)=old {
                    if old!=*body { return Err(Error::Conflict(format!("observation {}",record.id))); }
                    outcome.replayed+=1;continue;
                }
                if closed { return Err(Error::Closed); }
                crate::project::ensure_run_active(transaction,&self.run)?;
                if outcome.inserted==0 { outcome.revision=crate::project::bump_revision(transaction)?; }
                let time=record.time.as_ref();
                transaction.execute("INSERT INTO records(run,source,id,schema_name,schema_version,subject,clock,start,end,uncertainty,function,location,operation,body,body_bytes,revision) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",params![self.run,self.id,record.id,record.schema,record.schema_version,record.subject,time.map(|t|&t.clock),time.map(|t|t.start),time.and_then(|t|t.end),time.map(|t|t.uncertainty.to_string()),record.function,record.location,record.operation,body,body.len(),outcome.revision])?;
                let seq=transaction.last_insert_rowid();
                let schema=definition.schemas.iter().find(|schema|schema.name==record.schema && schema.version==record.schema_version).expect("validated schema");
                for (name,value) in record.payload.as_object().expect("validated object") {
                    if !schema.fields.get(name).is_some_and(|field|field.kind.supports_equality()) {continue;}
                    let value=crate::project::encode_bounded(value)?;
                    transaction.execute("INSERT INTO record_fields(record,name,value) VALUES (?1,?2,?3)",params![seq,name,value])?;
                }
                outcome.inserted+=1;
            }
            if let Some(delivery)=delivery {
                transaction.execute("INSERT INTO delivery_batches(run,source,id,digest,record_count,revision) VALUES (?1,?2,?3,?4,?5,?6)",params![self.run,self.id,delivery,digest,records.len(),outcome.revision])?;
            }
            Ok(outcome)
        })
    }

    pub fn close(&self, outcome: Value) -> Result<u64> {
        let body = crate::project::encode_bounded(&outcome)?;
        self.project.transaction(|transaction| {
            let (closed, previous, cutoff): (bool, Option<String>, Option<u64>) = transaction
                .query_row(
                    "SELECT closed,outcome,cutoff FROM sources WHERE run=?1 AND id=?2",
                    params![self.run, self.id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
            if closed {
                return if previous.as_deref() == Some(&body) {
                    cutoff.ok_or_else(|| Error::Invalid("closed source lacks cutoff".into()))
                } else {
                    Err(Error::Conflict(
                        "source already closed with another outcome".into(),
                    ))
                };
            }
            let revision = crate::project::bump_revision(transaction)?;
            transaction.execute(
                "UPDATE sources SET closed=1,outcome=?3,cutoff=?4 WHERE run=?1 AND id=?2",
                params![self.run, self.id, body, revision],
            )?;
            Ok(revision)
        })
    }
}

impl Project {
    pub fn register_source(
        &self,
        run: &Reference,
        definition: SourceDefinition,
    ) -> Result<SourceHandle> {
        self.check_reference(run)?;
        crate::project::validate_id(&definition.id)?;
        crate::project::validate_id(&definition.interpretation)?;
        if definition.schemas.len() > 64 || definition.clocks.len() > 64 {
            return Err(Error::Limit("source exceeds 64 schemas or clocks".into()));
        }
        let mut schemas = std::collections::BTreeSet::new();
        for schema in &definition.schemas {
            schema.validate_definition()?;
            if !schemas.insert((&schema.name, schema.version)) {
                return Err(Error::Invalid("duplicate source schema".into()));
            }
        }
        let mut clocks = std::collections::BTreeSet::new();
        for clock in &definition.clocks {
            crate::project::validate_id(&clock.id)?;
            if clock.ticks_per_second == 0
                || clock.description.is_empty()
                || !clocks.insert(&clock.id)
            {
                return Err(Error::Invalid(
                    "clock needs unique identity, nonzero rate and meaning".into(),
                ));
            }
        }
        if let Some(target) = &definition.target {
            if target.project != self.id() {
                return Err(Error::ForeignProject);
            }
            if target.run != run.id {
                return Err(Error::Conflict(
                    "source target belongs to another run".into(),
                ));
            }
            crate::project::validate_id(&target.id)?;
        }
        let body = crate::project::encode_bounded(&definition)?;
        self.transaction(|transaction| {
            crate::project::require_id(transaction,"runs",&run.id)?;
            if let Some(target)=&definition.target {
                let found=transaction.query_row("SELECT 1 FROM targets WHERE run=?1 AND id=?2",params![run.id,target.id],|_|Ok(())).optional()?.is_some();
                if !found { return Err(Error::NotFound(format!("target {}",target.id))); }
            }
            let existing:Option<String>=transaction.query_row("SELECT definition FROM sources WHERE run=?1 AND id=?2",params![run.id,definition.id],|row|row.get(0)).optional()?;
            if let Some(existing)=existing {
                if existing!=body { return Err(Error::Conflict(format!("source {}",definition.id))); }
            } else {
                crate::project::ensure_run_active(transaction,&run.id)?;
                let revision=crate::project::bump_revision(transaction)?;
                transaction.execute("INSERT INTO sources(run,id,definition,created_revision) VALUES (?1,?2,?3,?4)",params![run.id,definition.id,body,revision])?;
                for schema in &definition.schemas {
                    transaction.execute("INSERT INTO source_schemas(run,source,name,version,body) VALUES (?1,?2,?3,?4,?5)",params![run.id,definition.id,schema.name,schema.version,crate::project::encode_bounded(schema)?])?;
                }
            }
            Ok(SourceHandle { project:self.clone(),run:run.id.clone(),id:definition.id.clone() })
        })
    }

    pub fn source(&self, run: &Reference, id: &str) -> Result<SourceHandle> {
        self.check_reference(run)?;
        crate::project::validate_id(id)?;
        let handle = SourceHandle {
            project: self.clone(),
            run: run.id.clone(),
            id: id.to_owned(),
        };
        handle.definition()?;
        Ok(handle)
    }
}

fn validate_record(record: &Record, definition: &SourceDefinition) -> Result<()> {
    crate::project::validate_id(&record.id)?;
    crate::project::validate_id(&record.subject)?;
    for value in [&record.function, &record.location, &record.operation]
        .into_iter()
        .flatten()
    {
        if value.is_empty() || value.len() > 4096 || value.contains('\0') {
            return Err(Error::Invalid(
                "record dimension must contain 1..=4096 non-nul bytes".into(),
            ));
        }
    }
    let schema = definition
        .schemas
        .iter()
        .find(|schema| schema.name == record.schema && schema.version == record.schema_version)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "unregistered schema {}:{}",
                record.schema, record.schema_version
            ))
        })?;
    schema.validate_payload(&record.payload)?;
    if let Some(time) = &record.time {
        if !definition.clocks.iter().any(|clock| clock.id == time.clock) {
            return Err(Error::Invalid("record clock is unregistered".into()));
        }
        if time.end.is_some_and(|end| end < time.start) {
            return Err(Error::Invalid(
                "record interval ends before it starts".into(),
            ));
        }
    }
    Ok(())
}

impl RecordSchema {
    pub(crate) fn validate_definition(&self) -> Result<()> {
        crate::project::validate_id(&self.name)?;
        if self.version == 0 || self.fields.is_empty() || self.fields.len() > 256 {
            return Err(Error::Invalid(
                "schema needs a version and 1..=256 typed fields".into(),
            ));
        }
        if self.description.is_empty() || self.description.len() > 4096 {
            return Err(Error::Invalid(
                "schema description must contain 1..=4096 bytes".into(),
            ));
        }
        for (name, field) in &self.fields {
            let mut kind = &field.kind;
            let mut depth = 0;
            while let FieldType::Nullable(inner) = kind {
                depth += 1;
                if depth > 64 {
                    return Err(Error::Invalid("field type nesting exceeds 64".into()));
                }
                kind = inner;
            }
            crate::project::validate_id(name)?;
            if field.meaning.is_empty() || field.meaning.len() > 4096 {
                return Err(Error::Invalid(
                    "field meaning must contain 1..=4096 bytes".into(),
                ));
            }
            if field
                .unit
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 128)
            {
                return Err(Error::Invalid(
                    "field unit must contain 1..=128 bytes".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn validate_payload(&self, value: &Value) -> Result<()> {
        let object = value
            .as_object()
            .ok_or_else(|| Error::Invalid("record payload must be an object".into()))?;
        for (name, field) in &self.fields {
            let Some(value) = object.get(name) else {
                if field.required {
                    return Err(Error::Invalid(format!("missing field {name}")));
                }
                continue;
            };
            let valid = field.kind.accepts(value);
            if !valid {
                return Err(Error::Invalid(format!(
                    "field {name} has an incompatible type"
                )));
            }
        }
        Ok(())
    }
}

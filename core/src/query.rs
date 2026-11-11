use std::io::{Read, Seek, SeekFrom};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, params, params_from_iter, types::Value as SqlValue,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    project::{self, Error, Project, Reference, Result},
    records::{Record, RecordSchema, SourceHandle},
};

mod annotations;
mod catalog;
pub use annotations::*;
pub use catalog::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RecordRef {
    pub project: String,
    pub run: String,
    pub source: String,
    pub id: String,
}

/// matches a stored JSON scalar representation, including its numeric representation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldFilter {
    pub name: String,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeFilter {
    pub clock: String,
    pub start: i64,
    pub end: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Query {
    pub run: Reference,
    pub source: Option<String>,
    pub schema: Option<String>,
    pub subject: Option<String>,
    pub function: Option<String>,
    pub location: Option<String>,
    pub operation: Option<String>,
    pub time: Option<TimeFilter>,
    pub fields: Vec<FieldFilter>,
}

impl Query {
    pub fn new(run: Reference) -> Self {
        Self {
            run,
            source: None,
            schema: None,
            subject: None,
            function: None,
            location: None,
            operation: None,
            time: None,
            fields: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct QueryLimits {
    pub max_rows: usize,
    pub max_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct QueryControl {
    pub timeout: Duration,
    pub cancel: Arc<AtomicBool>,
}

impl Default for QueryControl {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(2),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(Clone)]
struct QueryBudget {
    deadline: Instant,
    cancel: Arc<AtomicBool>,
}

impl QueryControl {
    fn budget(&self) -> Result<QueryBudget> {
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or_else(|| Error::Invalid("query timeout exceeds clock range".into()))?;
        let budget = QueryBudget {
            deadline,
            cancel: self.cancel.clone(),
        };
        budget.check()?;
        Ok(budget)
    }
}

impl QueryBudget {
    fn expired(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || Instant::now() >= self.deadline
    }
    fn check(&self) -> Result<()> {
        if self.expired() {
            Err(Error::Interrupted("query deadline or cancellation".into()))
        } else {
            Ok(())
        }
    }
}

impl Default for QueryLimits {
    fn default() -> Self {
        Self {
            max_rows: 128,
            max_bytes: 1024 * 1024,
        }
    }
}

impl QueryLimits {
    fn validate(self) -> Result<()> {
        if !(1..=4096).contains(&self.max_rows)
            || !(1024..=4 * 1024 * 1024).contains(&self.max_bytes)
        {
            return Err(Error::Limit(
                "query requires 1..=4096 rows and 1 kib..=4 mib result budget".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Continuation {
    project: String,
    query: Query,
    revision: u64,
    after: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum StoredKind {
    Record,
    Context,
    Relationship,
    Investigation,
    Run,
    Source,
    SourceEvent,
}

impl StoredKind {
    fn table(&self) -> &'static str {
        match self {
            Self::Record => "records",
            Self::Context => "contexts",
            Self::Relationship => "relationships",
            Self::Investigation => "investigations",
            Self::Run => "runs",
            Self::Source => "sources",
            Self::SourceEvent => "source_events",
        }
    }
    fn column(&self) -> &'static str {
        match self {
            Self::Investigation => "metadata",
            Self::Run => "conditions",
            Self::Source => "definition",
            _ => "body",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueRef {
    pub project: String,
    pub run: String,
    pub id: String,
    pub source: Option<String>,
    pub bytes: usize,
    revision: u64,
    row: i64,
    kind: StoredKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordEntry {
    pub reference: RecordRef,
    pub record: Option<Record>,
    pub value: ValueRef,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordPage {
    pub revision: u64,
    pub records: Vec<RecordEntry>,
    pub continuation: Option<Continuation>,
}

#[derive(Clone, Debug)]
pub struct ValueChunk {
    pub bytes: Vec<u8>,
    pub next_offset: Option<usize>,
}

impl SourceHandle {
    pub fn record_ref(&self, id: impl Into<String>) -> RecordRef {
        RecordRef {
            project: self.project.id().to_owned(),
            run: self.run.clone(),
            source: self.id.clone(),
            id: id.into(),
        }
    }
}

impl Project {
    fn reader(&self, budget: &QueryBudget) -> Result<Connection> {
        budget.check()?;
        let mut connection = Connection::open_with_flags(
            self.directory().join("project.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        connection.busy_timeout(
            budget
                .deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(2)),
        )?;
        let interrupted = budget.clone();
        connection.progress_handler(1000, Some(move || interrupted.expired()))?;
        connection.execute_batch("PRAGMA temp_store=MEMORY; PRAGMA trusted_schema=OFF;")?;
        connection.set_limit(
            rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
            2 * 1024 * 1024,
        )?;
        Ok(connection)
    }

    fn check_record_ref(&self, reference: &RecordRef) -> Result<()> {
        if reference.project != self.id() {
            return Err(Error::ForeignProject);
        }
        for id in [&reference.run, &reference.source, &reference.id] {
            project::validate_id(id)?;
        }
        Ok(())
    }

    pub fn query(
        &self,
        query: &Query,
        limits: QueryLimits,
        cursor: Option<&Continuation>,
    ) -> Result<RecordPage> {
        self.query_with_control(query, limits, cursor, &QueryControl::default())
    }

    pub fn query_with_control(
        &self,
        query: &Query,
        limits: QueryLimits,
        cursor: Option<&Continuation>,
        control: &QueryControl,
    ) -> Result<RecordPage> {
        self.query_inner(query, limits, cursor, &control.budget()?)
            .map_err(query_error)
    }

    fn query_inner(
        &self,
        query: &Query,
        limits: QueryLimits,
        cursor: Option<&Continuation>,
        budget: &QueryBudget,
    ) -> Result<RecordPage> {
        self.check_reference(&query.run)?;
        limits.validate()?;
        if query.fields.len() > 16 {
            return Err(Error::Limit("query exceeds 16 field filters".into()));
        }
        for field in &query.fields {
            project::validate_id(&field.name)?;
            if project::encode_bounded(&field.value)?.len() > 4096 {
                return Err(Error::Limit("field filter exceeds 4 kib".into()));
            }
        }
        for dimension in [
            &query.source,
            &query.schema,
            &query.subject,
            &query.function,
            &query.location,
            &query.operation,
        ]
        .into_iter()
        .flatten()
        {
            if dimension.len() > 4096 {
                return Err(Error::Limit("query dimension exceeds 4 kib".into()));
            }
        }
        if let Some(time) = &query.time {
            project::validate_id(&time.clock)?;
            if time.end < time.start {
                return Err(Error::Invalid("query interval is reversed".into()));
            }
        }
        if let Some(cursor) = cursor {
            if cursor.project != self.id() {
                return Err(Error::ForeignProject);
            }
            if project::encode_bounded(&cursor.query)? != project::encode_bounded(query)?
                || cursor.after < 0
            {
                return Err(Error::Conflict(
                    "continuation belongs to another query".into(),
                ));
            }
        }
        let mut connection = self.reader(budget)?;
        let transaction = connection.transaction()?;
        project::require_id(&transaction, "runs", &query.run.id)?;
        let current = project::revision(&transaction)?;
        let revision = cursor.map_or(current, |cursor| cursor.revision);
        if revision > current {
            return Err(Error::Invalid("query view is unavailable".into()));
        }
        validate_field_filters(&transaction, query, revision, budget)?;
        validate_time_scope(&transaction, query, revision, budget)?;
        let mut after = cursor.map_or(0, |cursor| cursor.after);
        let continuation = Continuation {
            project: self.id().to_owned(),
            query: query.clone(),
            revision,
            after: i64::MAX,
        };
        let mut page = RecordPage {
            revision,
            records: Vec::new(),
            continuation: Some(continuation.clone()),
        };
        let mut used = encoded_size(&page)?;
        if used > limits.max_bytes {
            return Err(Error::Limit("query metadata exceeds page budget".into()));
        }
        let mut more = false;
        'windows: loop {
            budget.check()?;
            let upper:Option<i64>=transaction.query_row("SELECT max(seq) FROM (SELECT seq FROM records WHERE run=?1 AND revision<=?2 AND seq>?3 ORDER BY seq LIMIT 8192)",params![query.run.id,revision,after],|row|row.get(0))?;
            let Some(upper) = upper else {
                break;
            };
            let mut arguments = vec![
                SqlValue::Text(query.run.id.clone()),
                SqlValue::Integer(as_i64(revision)?),
                SqlValue::Integer(after),
                SqlValue::Integer(upper),
            ];
            let mut sql="SELECT r.seq,r.source,r.id,r.body_bytes FROM records r WHERE r.run=? AND r.revision<=? AND r.seq>? AND r.seq<=?".to_owned();
            for (column, value) in [
                ("source", &query.source),
                ("schema_name", &query.schema),
                ("subject", &query.subject),
                ("function", &query.function),
                ("location", &query.location),
                ("operation", &query.operation),
            ] {
                if let Some(value) = value {
                    sql.push_str(&format!(" AND r.{column}=?"));
                    arguments.push(SqlValue::Text(value.clone()));
                }
            }
            if let Some(time) = &query.time {
                sql.push_str(" AND r.clock=? AND r.start<=? AND coalesce(r.end,r.start)>=?");
                arguments.extend([
                    SqlValue::Text(time.clock.clone()),
                    SqlValue::Integer(time.end),
                    SqlValue::Integer(time.start),
                ]);
            }
            for field in &query.fields {
                sql.push_str(" AND r.seq IN (SELECT record FROM record_fields WHERE name=? AND value=? AND record IN (SELECT seq FROM records WHERE run=? AND seq>? AND seq<=?))");
                arguments.extend([
                    SqlValue::Text(field.name.clone()),
                    SqlValue::Text(project::encode_bounded(&field.value)?),
                    SqlValue::Text(query.run.id.clone()),
                    SqlValue::Integer(after),
                    SqlValue::Integer(upper),
                ]);
            }
            sql.push_str(" ORDER BY r.seq LIMIT ?");
            arguments.push(SqlValue::Integer((limits.max_rows + 1) as i64));
            let mut statement = transaction.prepare(&sql)?;
            let mut rows = statement.query(params_from_iter(arguments))?;
            while let Some(row) = rows.next()? {
                budget.check()?;
                if page.records.len() == limits.max_rows {
                    more = true;
                    break 'windows;
                }
                let seq: i64 = row.get(0)?;
                let source: String = row.get(1)?;
                let id: String = row.get(2)?;
                let bytes: usize = row.get(3)?;
                let reference = RecordRef {
                    project: self.id().to_owned(),
                    run: query.run.id.clone(),
                    source: source.clone(),
                    id: id.clone(),
                };
                let value = ValueRef {
                    project: self.id().to_owned(),
                    run: query.run.id.clone(),
                    source: Some(source),
                    id,
                    bytes,
                    revision,
                    row: seq,
                    kind: StoredKind::Record,
                };
                let mut entry = RecordEntry {
                    reference,
                    record: None,
                    value,
                };
                let minimum = encoded_size(&entry)? + 1;
                if used + minimum > limits.max_bytes {
                    if page.records.is_empty() {
                        return Err(Error::Limit("record reference exceeds page budget".into()));
                    }
                    more = true;
                    break 'windows;
                }
                if used + minimum + bytes <= limits.max_bytes {
                    let body: String = transaction.query_row(
                        "SELECT body FROM records WHERE seq=?1",
                        [seq],
                        |row| row.get(0),
                    )?;
                    entry.record = Some(serde_json::from_str(&body)?);
                }
                let cost = encoded_size(&entry)? + 1;
                if used + cost > limits.max_bytes {
                    return Err(Error::Limit("record encoding exceeds page budget".into()));
                }
                used += cost;
                after = seq;
                page.records.push(entry);
            }
            after = upper;
        }
        if more {
            page.continuation = Some(Continuation {
                after,
                ..continuation
            });
        } else {
            page.continuation = None;
        }
        debug_assert!(encoded_size(&page).is_ok_and(|size| size <= limits.max_bytes));
        Ok(page)
    }

    pub fn record(&self, reference: &RecordRef, view: Option<u64>) -> Result<Record> {
        self.check_record_ref(reference)?;
        let budget = QueryControl::default().budget()?;
        let mut connection = self.reader(&budget)?;
        let transaction = connection.transaction()?;
        let revision = checked_view(&transaction, view)?;
        let body: Option<String> = transaction
            .query_row(
                "SELECT body FROM records WHERE run=?1 AND source=?2 AND id=?3 AND revision<=?4",
                params![reference.run, reference.source, reference.id, revision],
                |row| row.get(0),
            )
            .optional()?;
        Ok(serde_json::from_str(&body.ok_or_else(|| {
            Error::NotFound(format!("record {}", reference.id))
        })?)?)
    }

    pub fn read_value(
        &self,
        value: &ValueRef,
        offset: usize,
        max_bytes: usize,
    ) -> Result<ValueChunk> {
        if value.project != self.id() {
            return Err(Error::ForeignProject);
        }
        if max_bytes == 0 || max_bytes > 1024 * 1024 {
            return Err(Error::Limit("value chunk requires 1 byte..=1 mib".into()));
        }
        let budget = QueryControl::default().budget()?;
        let mut connection = self.reader(&budget)?;
        let transaction = connection.transaction()?;
        checked_view(&transaction, Some(value.revision))?;
        let sql = match value.kind {
            StoredKind::Record => "SELECT run,id,source,revision FROM records WHERE seq=?1",
            StoredKind::Context => "SELECT run,id,NULL,revision FROM contexts WHERE seq=?1",
            StoredKind::Relationship => {
                "SELECT run,id,NULL,revision FROM relationships WHERE seq=?1"
            }
            StoredKind::Investigation => {
                "SELECT '',id,NULL,created_revision FROM investigations WHERE rowid=?1"
            }
            StoredKind::Run => "SELECT id,id,NULL,created_revision FROM runs WHERE rowid=?1",
            StoredKind::Source => "SELECT run,id,id,created_revision FROM sources WHERE rowid=?1",
            StoredKind::SourceEvent => {
                "SELECT run,id,source,revision FROM source_events WHERE seq=?1"
            }
        };
        let (run, id, source, revision): (String, String, Option<String>, u64) = transaction
            .query_row(sql, [value.row], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()?
            .ok_or_else(|| Error::NotFound("stored value".into()))?;
        if run != value.run || id != value.id || source != value.source || revision > value.revision
        {
            return Err(Error::Conflict(
                "value reference does not match retained evidence".into(),
            ));
        }
        let mut blob = transaction.blob_open(
            rusqlite::MAIN_DB,
            value.kind.table(),
            value.kind.column(),
            value.row,
            true,
        )?;
        let length = blob.len();
        if value.bytes != length || offset > length {
            return Err(Error::Invalid("value length or offset mismatch".into()));
        }
        let size = max_bytes.min(length - offset);
        let mut bytes = vec![0; size];
        blob.seek(SeekFrom::Start(offset as u64))?;
        blob.read_exact(&mut bytes)?;
        let next = offset + size;
        Ok(ValueChunk {
            bytes,
            next_offset: (next < length).then_some(next),
        })
    }
}

fn encoded_size(value: &impl Serialize) -> Result<usize> {
    Ok(serde_json::to_vec(value)?.len())
}

fn validate_field_filters(
    connection: &Connection,
    query: &Query,
    revision: u64,
    budget: &QueryBudget,
) -> Result<()> {
    if query.fields.is_empty() {
        return Ok(());
    }
    let mut found = vec![false; query.fields.len()];
    let mut statement=connection.prepare("SELECT ss.body FROM source_schemas ss JOIN sources s ON s.run=ss.run AND s.id=ss.source WHERE ss.run=?1 AND (?2 IS NULL OR ss.source=?2) AND (?3 IS NULL OR ss.name=?3) AND s.created_revision<=?4")?;
    let mut rows = statement.query(params![query.run.id, query.source, query.schema, revision])?;
    while let Some(row) = rows.next()? {
        budget.check()?;
        let schema: RecordSchema = serde_json::from_str(&row.get::<_, String>(0)?)?;
        for (index, filter) in query.fields.iter().enumerate() {
            if schema.fields.get(&filter.name).is_some_and(|field| {
                field.kind.supports_equality() && field.kind.accepts(&filter.value)
            }) {
                found[index] = true;
            }
        }
        if found.iter().all(|found| *found) {
            return Ok(());
        }
    }
    let missing = query
        .fields
        .iter()
        .zip(found)
        .filter_map(|(field, found)| (!found).then_some(field.name.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    Err(Error::Invalid(format!(
        "unsupported scalar field predicate for selected source schemas: {missing}"
    )))
}

fn validate_time_scope(
    connection: &Connection,
    query: &Query,
    revision: u64,
    budget: &QueryBudget,
) -> Result<()> {
    let Some(time) = &query.time else {
        return Ok(());
    };
    let mut statement=connection.prepare("SELECT s.definition FROM sources s WHERE s.run=?1 AND s.created_revision<=?2 AND (?3 IS NULL OR s.id=?3) AND (?4 IS NULL OR EXISTS(SELECT 1 FROM source_schemas ss WHERE ss.run=s.run AND ss.source=s.id AND ss.name=?4))")?;
    let mut rows = statement.query(params![query.run.id, revision, query.source, query.schema])?;
    let mut found = false;
    while let Some(row) = rows.next()? {
        budget.check()?;
        let definition: crate::records::SourceDefinition =
            serde_json::from_str(&row.get::<_, String>(0)?)?;
        if definition.clocks.iter().any(|clock| clock.id == time.clock) {
            if found {
                return Err(Error::Invalid(
                    "time predicate matches multiple source-local clocks; select one source".into(),
                ));
            }
            found = true;
        }
    }
    if !found {
        return Err(Error::Invalid(
            "time predicate names no registered clock in the selected source schemas".into(),
        ));
    }
    Ok(())
}
fn query_error(error: Error) -> Error {
    if matches!(&error,Error::Sqlite(rusqlite::Error::SqliteFailure(code,_)) if code.code==rusqlite::ErrorCode::OperationInterrupted)
    {
        Error::Interrupted("query deadline or cancellation".into())
    } else {
        error
    }
}
fn as_i64(value: u64) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| Error::Invalid("revision exceeds sqlite integer domain".into()))
}
fn checked_view(connection: &Connection, view: Option<u64>) -> Result<u64> {
    let current = project::revision(connection)?;
    let revision = view.unwrap_or(current);
    if revision > current {
        return Err(Error::Invalid("query view is unavailable".into()));
    }
    Ok(revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_sql_query_observes_deadline_and_external_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let project = Project::create(directory.path(), "p").unwrap();
        for cancelled in [false, true] {
            let control = QueryControl {
                timeout: if cancelled {
                    Duration::from_secs(2)
                } else {
                    Duration::from_millis(15)
                },
                cancel: Arc::new(AtomicBool::new(false)),
            };
            let budget = control.budget().unwrap();
            let reader = project.reader(&budget).unwrap();
            let signal = control.cancel.clone();
            let worker = cancelled.then(|| {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(15));
                    signal.store(true, Ordering::Relaxed);
                })
            });
            let start = Instant::now();
            let error=reader.query_row("WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n",[],|row|row.get::<_,i64>(0)).unwrap_err();
            assert!(matches!(query_error(error.into()), Error::Interrupted(_)));
            assert!(start.elapsed() < Duration::from_secs(1));
            if let Some(worker) = worker {
                worker.join().unwrap();
            }
        }
        let cancelled = QueryControl {
            timeout: Duration::from_secs(1),
            cancel: Arc::new(AtomicBool::new(true)),
        };
        assert!(matches!(
            project.query_with_control(
                &Query::new(project.reference("missing")),
                QueryLimits::default(),
                None,
                &cancelled
            ),
            Err(Error::Interrupted(_))
        ));
    }
}

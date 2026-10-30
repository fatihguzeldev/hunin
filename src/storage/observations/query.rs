//! Bounded selected-record reads. The private SQL transaction owns selection.
use super::super::Store;
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// Short WAL cleanup/recovery contention belongs to this read, not a replay.
const SQLITE_BUSY_WAIT: Duration = Duration::from_millis(250);

#[derive(Clone, Debug)]
pub struct ReadLimits {
    pub maximum_rows: usize,
    pub maximum_bytes: usize,
    pub maximum_record_bytes: usize,
    pub maximum_result_sets: usize,
    pub maximum_sqlite_steps: u64,
}
impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            maximum_rows: 100_000,
            maximum_bytes: 128 * 1024 * 1024,
            maximum_record_bytes: 8 * 1024 * 1024,
            maximum_result_sets: 10_000,
            maximum_sqlite_steps: 10_000_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadDimension {
    Rows,
    Bytes,
    RecordBytes,
    ResultSets,
    SqliteSteps,
}
impl ReadDimension {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rows => "rows",
            Self::Bytes => "selected_bytes",
            Self::RecordBytes => "record_bytes",
            Self::ResultSets => "result_sets",
            Self::SqliteSteps => "sqlite_steps",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReadBudgetExceeded {
    pub dimension: ReadDimension,
    pub limit: u64,
    pub observed: u64,
}
impl fmt::Display for ReadBudgetExceeded {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            output,
            "selected query {} budget exceeded: observed {}, limit {}",
            self.dimension.as_str(),
            self.observed,
            self.limit
        )
    }
}
impl std::error::Error for ReadBudgetExceeded {}

#[derive(Clone, Debug, Default)]
pub struct ReadUsage {
    pub rows: usize,
    /// Observation JSON bytes; identity text is separately accounted below.
    pub bytes: usize,
    pub identity_bytes: usize,
    pub result_sets: usize,
    pub sqlite_steps_approx: u64,
}

#[derive(Debug)]
pub struct QuerySnapshot<T> {
    pub value: T,
    pub result_identity: String,
    pub usage: ReadUsage,
}

/// A connection-local check shared by row visits and SQLite VM progress.
/// It must never issue SQL or re-enter itself while holding the checker mutex.
type SharedCheckpoint = Arc<Mutex<Box<dyn FnMut() -> Result<()> + Send>>>;

#[derive(Clone)]
struct ReadControl {
    checker: SharedCheckpoint,
    interruption: Arc<Mutex<Option<anyhow::Error>>>,
    steps: Arc<AtomicU64>,
}

impl ReadControl {
    fn new(
        connection: &Connection,
        maximum_steps: u64,
        checker: impl FnMut() -> Result<()> + Send + 'static,
    ) -> Self {
        let control = Self {
            checker: Arc::new(Mutex::new(Box::new(checker))),
            interruption: Arc::new(Mutex::new(None)),
            steps: Arc::new(AtomicU64::new(0)),
        };
        let callback = control.clone();
        let interval = maximum_steps.clamp(1, 1000);
        connection.progress_handler(
            interval as i32,
            Some(move || {
                let before = callback
                    .steps
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |steps| {
                        Some(steps.saturating_add(interval))
                    })
                    .unwrap();
                let observed = before.saturating_add(interval);
                let checked = if observed > maximum_steps {
                    Err(ReadBudgetExceeded {
                        dimension: ReadDimension::SqliteSteps,
                        limit: maximum_steps,
                        observed,
                    }
                    .into())
                } else {
                    callback.checkpoint()
                };
                if let Err(error) = checked {
                    let mut reason = callback.interruption.lock().unwrap();
                    if reason.is_none() {
                        *reason = Some(error);
                    }
                    true
                } else {
                    false
                }
            }),
        );
        control
    }

    fn checkpoint(&self) -> Result<()> {
        (self
            .checker
            .lock()
            .map_err(|_| anyhow::anyhow!("query checkpoint mutex poisoned"))?)()
    }

    fn sql<T>(&self, result: rusqlite::Result<T>) -> Result<T> {
        if let Some(reason) = self.interruption.lock().unwrap().take() {
            return Err(reason);
        }
        // SQLite's supported busy timeout is one non-preemptible locking unit.
        // Preserve the request's original typed error if its interest expired
        // during that wait, including when SQLite ultimately returns BUSY.
        self.checkpoint()?;
        result.map_err(Into::into)
    }
}

pub struct QueryReadPort<'a> {
    connection: &'a Connection,
    run_id: &'a str,
    control: ReadControl,
    limits: ReadLimits,
    usage: ReadUsage,
}

fn within(dimension: ReadDimension, limit: usize, observed: usize) -> Result<()> {
    if observed > limit {
        return Err(ReadBudgetExceeded {
            dimension,
            limit: limit as u64,
            observed: observed as u64,
        }
        .into());
    }
    Ok(())
}

impl QueryReadPort<'_> {
    pub fn checkpoint(&self) -> Result<()> {
        self.control.checkpoint()
    }

    /// A pure checker for work inside one decoded record. It shares the same
    /// request control as SQL progress without exposing the database handle.
    pub fn checkpointer(&self) -> impl FnMut() -> Result<()> + Send + Clone + 'static {
        let control = self.control.clone();
        move || control.checkpoint()
    }

    /// Kinds filter on stored columns before JSON materialization. Limits count
    /// selected rows/bytes cumulatively across visits. The VM counter also
    /// bounds skipped historical rows, joins and pre-first-row SQL work.
    pub fn visit(&mut self, kinds: &[&str], visitor: impl FnMut(&str) -> Result<()>) -> Result<()> {
        self.visit_selected(kinds, None, None, visitor)
    }

    /// Select a context/relation page before copying payloads into Rust. VM
    /// work for skipped/sorted rows still consumes the same snapshot budget.
    pub fn visit_page(
        &mut self,
        kinds: &[&str],
        limit: usize,
        offset: usize,
        visitor: impl FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        self.visit_selected(kinds, None, Some((limit, offset)), visitor)
    }

    /// Requested evidence IDs remain opaque; duplicates/missing IDs retain SQL
    /// set semantics and selected timestamp/sequence ordering.
    pub fn visit_ids(
        &mut self,
        ids: &[String],
        visitor: impl FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        self.visit_selected(&[], Some(ids), None, visitor)
    }

    fn visit_selected(
        &mut self,
        kinds: &[&str],
        ids: Option<&[String]>,
        page: Option<(usize, usize)>,
        mut visitor: impl FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        self.checkpoint()?;
        let connection = self.connection;
        // Full timelines stream in timestamp/sequence order. Kind selectors
        // seek through the kind index so unrelated sample-heavy data stays out
        // of resource/context work. Sort only scalar keys, then fetch each
        // payload by primary key after its row sentinel. Sorting raw records
        // could otherwise spool many large values before any byte check.
        // The shared VM budget covers both the key sort and payload lookups.
        let scalar_keys = !kinds.is_empty() || ids.is_some() || page.is_some();
        let mut arguments: Vec<rusqlite::types::Value> = vec![self.run_id.to_owned().into()];
        let mut sql = if let Some(ids) = ids {
            arguments.push(serde_json::to_string(ids)?.into());
            "SELECT o.seq FROM observations o JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND r.active=1 AND o.id IN(SELECT value FROM json_each(?2)) ORDER BY o.timestamp_us,o.seq".to_owned()
        } else if !kinds.is_empty() {
            arguments.push(serde_json::to_string(kinds)?.into());
            "SELECT o.seq FROM observations o INDEXED BY observation_kind JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND r.active=1 AND o.kind IN(SELECT value FROM json_each(?2)) ORDER BY o.timestamp_us,o.seq".to_owned()
        } else if scalar_keys {
            "SELECT o.seq FROM observations o INDEXED BY observation_timeline JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND r.active=1 ORDER BY o.timestamp_us,o.seq".to_owned()
        } else {
            "SELECT o.record FROM observations o INDEXED BY observation_timeline JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND r.active=1 ORDER BY o.timestamp_us,o.seq".to_owned()
        };
        if let Some((limit, offset)) = page {
            let parameter = arguments.len() + 1;
            sql.push_str(&format!(" LIMIT ?{parameter} OFFSET ?{}", parameter + 1));
            arguments.push(
                i64::try_from(limit)
                    .context("query page limit outside SQLite range")?
                    .into(),
            );
            arguments.push(
                i64::try_from(offset)
                    .context("query page offset outside SQLite range")?
                    .into(),
            );
        }
        let mut statement = self.control.sql(connection.prepare(&sql))?;
        let mut rows = self
            .control
            .sql(statement.query(rusqlite::params_from_iter(arguments)))?;
        let mut lookup = None;
        loop {
            self.checkpoint()?;
            let Some(row) = self.control.sql(rows.next())? else {
                break;
            };
            let observed = self.usage.rows.saturating_add(1);
            within(ReadDimension::Rows, self.limits.maximum_rows, observed)?;
            let mut payload_rows = if !scalar_keys {
                None
            } else {
                if lookup.is_none() {
                    lookup =
                        Some(self.control.sql(
                            connection.prepare("SELECT record FROM observations WHERE seq=?1"),
                        )?);
                }
                Some(
                    self.control
                        .sql(lookup.as_mut().unwrap().query([row.get::<_, i64>(0)?]))?,
                )
            };
            let row = if let Some(payloads) = &mut payload_rows {
                self.control
                    .sql(payloads.next())?
                    .context("selected observation disappeared inside its read snapshot")?
            } else {
                row
            };
            let text = row.get_ref(0)?.as_str()?;
            within(
                ReadDimension::RecordBytes,
                self.limits.maximum_record_bytes,
                text.len(),
            )?;
            let bytes = self.usage.bytes.saturating_add(text.len());
            within(
                ReadDimension::Bytes,
                self.limits.maximum_bytes,
                bytes.saturating_add(self.usage.identity_bytes),
            )?;
            self.usage.rows = observed;
            self.usage.bytes = bytes;
            self.checkpoint()?;
            visitor(text)?;
        }
        self.checkpoint()
    }
}

impl Store {
    /// Budgets count serialized selection work and approximate SQLite VM work,
    /// not process RSS or an SLA. No single I/O/SQLite/JSON unit is preemptible.
    /// Checkpoints must be pure controls: no SQL and no checker re-entry.
    /// Normal live-WAL semantics can use SQLite sidecars; this is not forensic
    /// zero-filesystem-write access. The retained schema/content is read-only.
    /// Each SQLite locking unit may wait 250ms for transient contention; the
    /// original request checkpoint is rechecked afterward. Rows are not replayed.
    pub fn with_query_snapshot<T>(
        &self,
        run_id: &str,
        limits: ReadLimits,
        mut checkpoint: impl FnMut() -> Result<()> + Send + 'static,
        consume: impl FnOnce(&mut QueryReadPort<'_>) -> Result<T>,
    ) -> Result<QuerySnapshot<T>> {
        checkpoint()?;
        let mut connection = Connection::open_with_flags(
            self.root.join("hunin.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.busy_timeout(SQLITE_BUSY_WAIT)?;
        let control = ReadControl::new(&connection, limits.maximum_sqlite_steps, checkpoint);
        control.checkpoint()?;
        let tx = control.sql(connection.transaction())?;
        let identity = self.validate_identity(&tx);
        // Preserve the original budget/cancellation origin if identity SQL was interrupted.
        control.sql(Ok(()))?;
        identity?;
        let schema: u32 = control.sql(tx.query_row("PRAGMA user_version", [], |row| row.get(0)))?;
        anyhow::ensure!(
            schema <= super::super::SCHEMA_VERSION,
            "recording database schema {schema} requires a newer Hunin; query reader supports up to {}",
            super::super::SCHEMA_VERSION
        );
        control
            .sql(
                tx.query_row("SELECT id FROM runs WHERE id=?1", [run_id], |_| Ok(()))
                    .optional(),
            )?
            .context("recording not found")?;
        let mut usage = ReadUsage::default();
        let mut identities = Vec::new();
        {
            let mut statement = control.sql(
                tx.prepare("SELECT id FROM result_sets WHERE run_id=?1 AND active=1 ORDER BY id"),
            )?;
            let mut rows = control.sql(statement.query([run_id]))?;
            loop {
                control.checkpoint()?;
                let Some(row) = control.sql(rows.next())? else {
                    break;
                };
                let count = usage.result_sets.saturating_add(1);
                within(ReadDimension::ResultSets, limits.maximum_result_sets, count)?;
                let identity = row.get_ref(0)?.as_str()?;
                let bytes = usage.identity_bytes.saturating_add(identity.len());
                within(ReadDimension::Bytes, limits.maximum_bytes, bytes)?;
                usage.result_sets = count;
                usage.identity_bytes = bytes;
                identities.push(identity.to_owned());
            }
        }
        let result_identity = format!(
            "selected-results-v1:{:x}",
            Sha256::digest(serde_json::to_vec(&(run_id, &identities))?)
        );
        let mut port = QueryReadPort {
            connection: &tx,
            run_id,
            control: control.clone(),
            limits,
            usage,
        };
        let value = consume(&mut port)?;
        let mut usage = port.usage;
        control.checkpoint()?;
        control.sql(tx.commit())?;
        usage.sqlite_steps_approx = control.steps.load(Ordering::Relaxed);
        Ok(QuerySnapshot {
            value,
            result_identity,
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    #[derive(Debug)]
    struct Cancelled;
    impl fmt::Display for Cancelled {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("request cancelled")
        }
    }
    impl std::error::Error for Cancelled {}

    #[test]
    fn sqlite_interrupt_preserves_the_original_checkpoint_error() {
        let connection = Connection::open_in_memory().unwrap();
        let control = ReadControl::new(&connection, u64::MAX, || Err(Cancelled.into()));
        let result=connection.query_row("WITH RECURSIVE numbers(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM numbers WHERE n<10000) SELECT SUM(n) FROM numbers",[],|row|row.get::<_,i64>(0));
        let error = control.sql(result).unwrap_err();
        assert!(error.downcast_ref::<Cancelled>().is_some(), "{error:#}");
    }

    #[test]
    fn kind_sort_and_row_sentinel_do_not_read_raw_payloads() {
        use rusqlite::hooks::{AuthAction, Authorization};
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE observations(seq INTEGER PRIMARY KEY,run_id TEXT,kind TEXT,timestamp_us INTEGER,result_set_id TEXT,record TEXT); CREATE INDEX observation_timeline ON observations(run_id,timestamp_us,seq); CREATE INDEX observation_kind ON observations(run_id,kind); CREATE TABLE result_sets(id TEXT PRIMARY KEY,active INTEGER); INSERT INTO result_sets VALUES('active',1)").unwrap();
        for timestamp in [3, 1, 2] {
            connection.execute("INSERT INTO observations(run_id,kind,timestamp_us,result_set_id,record) VALUES('recording','metric',?1,'active',?2)",params![timestamp,"invalid JSON".repeat(10000)]).unwrap();
        }
        let control = ReadControl::new(&connection, 10000, || Ok(()));
        // Authorization is checked at preparation, so this is stronger than
        // measuring memory: the sorted-key cursor cannot even request a raw
        // payload column before the row sentinel rejects the first row.
        connection.authorizer(Some(
            |context: rusqlite::hooks::AuthContext<'_>| match context.action {
                AuthAction::Read {
                    table_name: "observations",
                    column_name: "record",
                } => Authorization::Deny,
                _ => Authorization::Allow,
            },
        ));
        let mut port = QueryReadPort {
            connection: &connection,
            run_id: "recording",
            control,
            limits: ReadLimits {
                maximum_rows: 0,
                ..Default::default()
            },
            usage: ReadUsage::default(),
        };
        let error = port
            .visit(&["metric"], |_| {
                panic!("the row sentinel must precede payload decoding")
            })
            .unwrap_err();
        let budget = error
            .downcast_ref::<ReadBudgetExceeded>()
            .expect("kind sort requested raw payloads before the row sentinel");
        assert_eq!(budget.dimension, ReadDimension::Rows);
        assert_eq!(budget.observed, 1);
    }
}

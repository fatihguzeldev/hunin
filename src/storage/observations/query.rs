//! Bounded selected-record reads. The private SQL transaction owns selection.
use super::super::Store;
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

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
        result.map_err(|error| {
            self.interruption
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| error.into())
        })
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
    pub fn visit(
        &mut self,
        kinds: &[&str],
        mut visitor: impl FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        self.checkpoint()?;
        let connection = self.connection;
        // Full timelines stream in timestamp/sequence order. Kind selectors
        // seek through the kind index so unrelated sample-heavy data stays out
        // of resource/context work. The VM budget covers the matched-row sort.
        let sql = if kinds.is_empty() {
            "SELECT o.record FROM observations o INDEXED BY observation_timeline JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND r.active=1 ORDER BY o.timestamp_us,o.seq"
        } else {
            "SELECT o.record FROM observations o INDEXED BY observation_kind JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND r.active=1 AND o.kind IN(SELECT value FROM json_each(?2)) ORDER BY o.timestamp_us,o.seq"
        };
        let mut statement = self.control.sql(connection.prepare(sql))?;
        let mut rows = if kinds.is_empty() {
            self.control.sql(statement.query([self.run_id]))?
        } else {
            self.control
                .sql(statement.query(params![self.run_id, serde_json::to_string(kinds)?]))?
        };
        loop {
            self.checkpoint()?;
            let Some(row) = self.control.sql(rows.next())? else {
                break;
            };
            let observed = self.usage.rows.saturating_add(1);
            within(ReadDimension::Rows, self.limits.maximum_rows, observed)?;
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
        connection.busy_timeout(Duration::ZERO)?;
        let control = ReadControl::new(&connection, limits.maximum_sqlite_steps, checkpoint);
        control.checkpoint()?;
        let tx = control.sql(connection.transaction())?;
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
}

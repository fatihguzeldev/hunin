use super::{ExpectedIdentity, Store};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::{fs, path::Path, time::Duration};

/// Current migration target and highest schema this executable understands.
pub const SCHEMA_VERSION: u32 = 2;

impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root.join("artifacts"))?;
        let store = Self {
            root: root.canonicalize()?,
            identity: ExpectedIdentity::Any,
        };
        store.bootstrap()
    }

    pub(crate) fn open_project(root: &Path, project_id: &str) -> Result<Self> {
        Self::open_existing(root, ExpectedIdentity::Project(project_id.to_owned()))
    }

    pub(crate) fn open_legacy(root: &Path) -> Result<Self> {
        Self::open_existing(root, ExpectedIdentity::Unassigned)
    }

    fn open_existing(root: &Path, identity: ExpectedIdentity) -> Result<Self> {
        Self {
            root: root.canonicalize()?,
            identity,
        }
        .bootstrap()
    }

    fn bootstrap(self) -> Result<Self> {
        let mut connection = self.connection()?;
        let c = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        migrate(c)?;
        Ok(self)
    }

    pub(super) fn connection(&self) -> Result<Connection> {
        let flags = match self.identity {
            ExpectedIdentity::Any => {
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
            }
            _ => OpenFlags::SQLITE_OPEN_READ_WRITE,
        };
        let c = Connection::open_with_flags(self.root.join("hunin.sqlite"), flags).context(
            "selected recording database is unavailable; restore its store before retrying",
        )?;
        c.busy_timeout(Duration::from_secs(10))?;
        // Bind the actual opened connection before PRAGMAs, migration or writes.
        // A retained engine must not follow a reused path into another project.
        self.validate_identity(&c)?;
        c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        Ok(c)
    }

    pub(super) fn validate_identity(&self, c: &Connection) -> Result<()> {
        match &self.identity {
            ExpectedIdentity::Any => {}
            ExpectedIdentity::Unassigned => {
                let assigned: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='project_identity')",
                    [], |row| row.get(0),
                )?;
                ensure!(
                    !assigned,
                    "legacy store was adopted as a project; select that project and restart Hunin"
                );
            }
            ExpectedIdentity::Project(expected) => {
                let actual: String = c
                    .query_row(
                        "SELECT id FROM project_identity WHERE singleton=1",
                        [],
                        |row| row.get(0),
                    )
                    .context("selected store no longer has its project identity")?;
                ensure!(
                    &actual == expected,
                    "project store identity mismatch; restore its registered store and restart Hunin"
                );
            }
        }
        Ok(())
    }
}

fn migrate(c: rusqlite::Transaction<'_>) -> Result<()> {
    // The writer lock serializes this decision with any other schema writer.
    // A version observed before acquiring it cannot authorize a downgrade.
    let version: u32 = c.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    anyhow::ensure!(
        version <= SCHEMA_VERSION,
        "recording database schema {version} requires a newer Hunin; this executable supports up to {SCHEMA_VERSION}"
    );
    c.execute_batch(include_str!("schema.sql"))?;
    let columns: Vec<String> = c
        .prepare("PRAGMA table_info(artifacts)")?
        .query_map([], |r| r.get(1))?
        .collect::<rusqlite::Result<_>>()?;
    if !columns.iter().any(|name| name == "sha256") {
        c.execute_batch("ALTER TABLE artifacts ADD COLUMN sha256 TEXT; UPDATE artifacts SET sha256=json_extract(metadata,'$.sha256');")?;
    }
    c.execute_batch("CREATE INDEX IF NOT EXISTS artifact_blob ON artifacts(sha256);")?;
    let columns: Vec<String> = c
        .prepare("PRAGMA table_info(result_sets)")?
        .query_map([], |r| r.get(1))?
        .collect::<rusqlite::Result<_>>()?;
    if !columns.iter().any(|name| name == "source_key") {
        c.execute_batch("ALTER TABLE result_sets ADD COLUMN source_key TEXT NOT NULL DEFAULT ''; UPDATE result_sets SET source_key=input_identity;")?;
    }
    if !columns.iter().any(|name| name == "active") {
        c.execute_batch("ALTER TABLE result_sets ADD COLUMN active INTEGER NOT NULL DEFAULT 1;")?;
    }
    let columns: Vec<String> = c
        .prepare("PRAGMA table_info(documents)")?
        .query_map([], |r| r.get(1))?
        .collect::<rusqlite::Result<_>>()?;
    if !columns.iter().any(|name| name == "owner_run_id") {
        c.execute_batch("ALTER TABLE documents ADD COLUMN owner_run_id TEXT REFERENCES runs(id) ON DELETE CASCADE;")?;
    }
    c.execute_batch("UPDATE documents SET owner_run_id=json_extract(payload,'$.run_id') WHERE namespace IN('analysis-results','cancellations','profile-inputs') AND owner_run_id IS NULL AND json_extract(payload,'$.run_id') IN(SELECT id FROM runs);")?;
    c.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    c.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_transaction_rejects_newer_schema_without_altering_it() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA user_version=99;").unwrap();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert!(migrate(transaction).is_err());
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 99);
        let tables: u32 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0);
    }
}

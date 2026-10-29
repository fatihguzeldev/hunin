use super::Store;
use anyhow::Result;
use rusqlite::{Connection, TransactionBehavior};
use std::{fs, path::Path, time::Duration};

/// Current migration target and highest schema this executable understands.
pub const SCHEMA_VERSION: u32 = 2;

impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root.join("artifacts"))?;
        let store = Self {
            root: root.canonicalize()?,
        };
        let mut connection = store.connection()?;
        let c = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        migrate(c)?;
        Ok(store)
    }

    pub(super) fn connection(&self) -> Result<Connection> {
        let c = Connection::open(self.root.join("hunin.sqlite"))?;
        c.busy_timeout(Duration::from_secs(10))?;
        c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        Ok(c)
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
    c.execute_batch("UPDATE documents SET owner_run_id=json_extract(payload,'$.run_id') WHERE namespace IN('analysis-results','cancellations') AND owner_run_id IS NULL AND json_extract(payload,'$.run_id') IN(SELECT id FROM runs);")?;
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

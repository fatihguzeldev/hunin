//! The selected store owns its identity; registry paths are discovery metadata.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use std::{path::Path, time::Duration};

#[derive(Debug)]
pub(super) struct Identity {
    pub id: String,
    pub name: String,
}

pub(super) fn read(root: &Path) -> Result<Option<Identity>> {
    let connection =
        Connection::open_with_flags(root.join("hunin.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("project store is unavailable: {}", root.display()))?;
    connection.busy_timeout(Duration::from_secs(10))?;
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='project_identity')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    let identity = connection
        .query_row(
            "SELECT id,name FROM project_identity WHERE singleton=1",
            [],
            |row| {
                Ok(Identity {
                    id: row.get(0)?,
                    name: row.get(1)?,
                })
            },
        )
        .optional()?;
    ensure!(
        identity.is_some(),
        "project store has an empty identity table"
    );
    if let Some(identity) = &identity {
        ensure!(
            uuid::Uuid::parse_str(&identity.id).is_ok(),
            "project store has an invalid project ID"
        );
        ensure!(
            !identity.name.trim().is_empty(),
            "project store has an invalid project name"
        );
    }
    Ok(identity)
}

/// Called only for a new store or explicit adoption of an existing store.
pub(super) fn assign(root: &Path, id: &str, name: &str) -> Result<Identity> {
    let mut connection =
        Connection::open_with_flags(root.join("hunin.sqlite"), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.busy_timeout(Duration::from_secs(10))?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS project_identity (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            id TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL
        );",
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO project_identity(singleton,id,name) VALUES(1,?1,?2)",
        params![id, name],
    )?;
    let identity = tx.query_row(
        "SELECT id,name FROM project_identity WHERE singleton=1",
        [],
        |row| {
            Ok(Identity {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        },
    )?;
    tx.commit()?;
    Ok(identity)
}

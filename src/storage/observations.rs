//! Selected observation reads and transaction-scoped snapshots.
mod query;
use super::Store;
use crate::model::runs::{Artifact, Run};
use anyhow::{Context, Result};
pub use query::{
    QueryReadPort, QuerySnapshot, ReadBudgetExceeded, ReadDimension, ReadLimits, ReadUsage,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;

#[derive(Debug)]
pub(crate) struct RecordReadLimit {
    pub maximum_bytes: usize,
    pub actual_bytes: usize,
}
impl fmt::Display for RecordReadLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "selected observation is {} bytes; read limit is {}",
            self.actual_bytes, self.maximum_bytes
        )
    }
}
impl std::error::Error for RecordReadLimit {}

/// The connection stays private; consumers can inspect selected identities and
/// visit one decoded observation at a time within the same read transaction.
pub(crate) struct SelectedRecords<'a> {
    connection: &'a Connection,
    run_id: &'a str,
    identities: Vec<String>,
}

impl SelectedRecords<'_> {
    pub fn run(&self) -> Result<Run> {
        let text: String = self.connection.query_row(
            "SELECT manifest FROM runs WHERE id=?1",
            [self.run_id],
            |row| row.get(0),
        )?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn artifacts(&self, maximum_bytes: usize) -> Result<Vec<Artifact>> {
        let mut query = self
            .connection
            .prepare("SELECT id,metadata,sha256 FROM artifacts WHERE run_id=?1 ORDER BY id")?;
        let mut rows = query.query([self.run_id])?;
        let mut bytes = 0usize;
        let mut artifacts = Vec::new();
        while let Some(row) = rows.next()? {
            let text = row.get_ref(1)?.as_str()?;
            bytes = bytes.saturating_add(text.len());
            anyhow::ensure!(
                bytes <= maximum_bytes,
                "artifact metadata snapshot exceeds {maximum_bytes} bytes"
            );
            artifacts.push(super::artifacts::decode_metadata(
                text,
                row.get_ref(2)?.as_str()?,
                row.get_ref(0)?.as_str()?,
            )?);
        }
        Ok(artifacts)
    }
    pub fn result_ids(&self) -> &[String] {
        &self.identities
    }

    pub fn visit(
        &self,
        maximum_record_bytes: usize,
        visitor: impl FnMut(Value) -> Result<()>,
    ) -> Result<()> {
        visit_records(
            self.connection,
            self.run_id,
            &self.identities,
            maximum_record_bytes,
            visitor,
        )
    }
}

/// A selected-observation read snapshot. Its identity does not cover future
/// independent queries, mutable run status, or unrelated stored documents.
pub struct RecordsSnapshot {
    pub result_identity: String,
    pub records: Vec<Value>,
}

impl Store {
    pub(crate) fn with_selected_snapshot<T>(
        &self,
        run_id: &str,
        exclude_source_key: &str,
        consume: impl FnOnce(&SelectedRecords<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut connection = self.connection()?;
        let tx = connection.transaction()?;
        tx.query_row("SELECT id FROM runs WHERE id=?1", [run_id], |_| Ok(()))
            .optional()?
            .context("recording not found")?;
        let selected = SelectedRecords {
            connection: &tx,
            run_id,
            identities: active_result_ids(&tx, run_id, exclude_source_key)?,
        };
        let result = consume(&selected)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn records(&self, run_id: &str) -> Result<Vec<Value>> {
        self.run(run_id)?;
        let c = self.connection()?;
        selected_records(&c, run_id)
    }

    pub fn records_snapshot(&self, run_id: &str) -> Result<RecordsSnapshot> {
        let mut connection = self.connection()?;
        let tx = connection.transaction()?;
        tx.query_row("SELECT id FROM runs WHERE id=?1", [run_id], |_| Ok(()))
            .optional()?
            .context("recording not found")?;
        let identities: Vec<String> = tx
            .prepare("SELECT id FROM result_sets WHERE run_id=?1 AND active=1 ORDER BY id")?
            .query_map([run_id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let records = selected_records(&tx, run_id)?;
        tx.commit()?;
        // Immutable result IDs identify accepted bytes without hashing the
        // entire recording a second time. Run identity scopes the empty case.
        let identity = Sha256::digest(serde_json::to_vec(&(run_id, identities))?);
        Ok(RecordsSnapshot {
            result_identity: format!("selected-results-v1:{identity:x}"),
            records,
        })
    }

    pub fn records_history(&self, run_id: &str) -> Result<Vec<Value>> {
        self.run(run_id)?;
        let c = self.connection()?;
        let mut statement =
            c.prepare("SELECT record FROM observations WHERE run_id=?1 ORDER BY seq")?;
        statement
            .query_map([run_id], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }

    /// Snapshot candidates omit the analyzer's old results and their dependents.
    pub fn active_result_ids(&self, run_id: &str, exclude_source_key: &str) -> Result<Vec<String>> {
        self.run(run_id)?;
        let c = self.connection()?;
        active_result_ids(&c, run_id, exclude_source_key)
    }

    pub fn records_for_result_ids(&self, run_id: &str, ids: &[String]) -> Result<Vec<Value>> {
        self.run(run_id)?;
        let c = self.connection()?;
        let mut result = Vec::new();
        visit_records(&c, run_id, ids, usize::MAX, |record| {
            result.push(record);
            Ok(())
        })?;
        Ok(result)
    }

    pub fn selected_result_id(&self, run_id: &str, source_key: &str) -> Result<Option<String>> {
        Ok(self
            .connection()?
            .query_row(
                "SELECT id FROM result_sets WHERE run_id=?1 AND source_key=?2 AND active=1",
                params![run_id, source_key],
                |r| r.get(0),
            )
            .optional()?)
    }
}

fn active_result_ids(
    connection: &Connection,
    run_id: &str,
    exclude_source_key: &str,
) -> Result<Vec<String>> {
    let mut query = connection.prepare("WITH RECURSIVE omitted(id) AS (SELECT id FROM result_sets WHERE run_id=?1 AND source_key=?2 UNION SELECT d.result_set_id FROM result_set_dependencies d JOIN omitted o ON d.input_result_set_id=o.id) SELECT id FROM result_sets WHERE run_id=?1 AND active=1 AND id NOT IN(SELECT id FROM omitted) ORDER BY id")?;
    Ok(query
        .query_map(params![run_id, exclude_source_key], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

fn visit_records(
    connection: &Connection,
    run_id: &str,
    ids: &[String],
    maximum_record_bytes: usize,
    mut visitor: impl FnMut(Value) -> Result<()>,
) -> Result<()> {
    let mut query = connection.prepare("SELECT o.record FROM observations o JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND r.active=1 AND o.result_set_id IN(SELECT value FROM json_each(?2)) ORDER BY o.timestamp_us,o.seq")?;
    let mut rows = query.query(params![run_id, serde_json::to_string(ids)?])?;
    while let Some(row) = rows.next()? {
        let text = row.get_ref(0)?.as_str()?;
        if text.len() > maximum_record_bytes {
            return Err(RecordReadLimit {
                maximum_bytes: maximum_record_bytes,
                actual_bytes: text.len(),
            }
            .into());
        }
        visitor(serde_json::from_str(text)?)?;
    }
    Ok(())
}

fn selected_records(connection: &Connection, run_id: &str) -> Result<Vec<Value>> {
    let mut statement = connection.prepare("SELECT o.record FROM observations o JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND r.active=1 ORDER BY o.timestamp_us,o.seq")?;
    statement
        .query_map([run_id], |row| row.get::<_, String>(0))?
        .map(|row| Ok(serde_json::from_str(&row?)?))
        .collect()
}

#[cfg(test)]
mod metadata_snapshot_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn analyzer_snapshot_keeps_manifest_artifacts_and_observations_on_one_commit() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let mut run: crate::model::runs::Run = serde_json::from_value(json!({"id":"snapshot","name":"snapshot","status":"completed","command":[],"cwd":".","runtime":"none","started_at_us":1,"finished_at_us":2,"exit_code":0,"diagnostics":[],"source":{"fingerprint":"before"},"correctness":{}})).unwrap();
        store.save_run(&run).unwrap();
        let original = store
            .add_artifact_bytes("snapshot", "before.bin", "raw", b"before")
            .unwrap();
        store
            .with_selected_snapshot("snapshot", "excluded", |selected| {
                run.source["fingerprint"] = json!("after");
                store.save_run(&run)?;
                store.add_artifact_bytes("snapshot", "after.bin", "raw", b"after")?;
                assert_eq!(selected.run()?.source["fingerprint"], "before");
                let artifacts = selected.artifacts(1024)?;
                assert_eq!(artifacts.len(), 1);
                assert_eq!(artifacts[0].id, original.id);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            store.run("snapshot").unwrap().source["fingerprint"],
            "after"
        );
        assert_eq!(store.artifacts("snapshot").unwrap().len(), 2);
    }
}

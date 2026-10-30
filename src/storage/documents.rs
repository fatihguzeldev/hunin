//! Stored documents, owning-run indexes and serialized analyzer cancellation.
use super::{Store, metadata};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;

impl Store {
    pub fn document(&self, namespace: &str, id: &str) -> Result<Value> {
        let text: String = self
            .connection()?
            .query_row(
                "SELECT payload FROM documents WHERE namespace=?1 AND id=?2",
                params![namespace, id],
                |r| r.get(0),
            )
            .context("document not found")?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn documents(&self, namespace: &str) -> Result<Vec<Value>> {
        let c = self.connection()?;
        let mut statement = c.prepare(
            "SELECT payload FROM documents WHERE namespace=?1 ORDER BY updated_at_us DESC,id",
        )?;
        statement
            .query_map([namespace], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }

    pub(crate) fn document_for_run(
        &self,
        namespace: &str,
        id: &str,
        run_id: &str,
    ) -> Result<Option<Value>> {
        let text: Option<String> = self
            .connection()?
            .query_row(
                "SELECT payload FROM documents WHERE namespace=?1 AND id=?2 AND owner_run_id=?3",
                params![namespace, id, run_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(text.map(|text| serde_json::from_str(&text)).transpose()?)
    }

    /// Page scalar keys before bounded JSON decode; completed attempts keep
    /// creation order, including the existing timestamp fallback.
    pub fn documents_page(
        &self,
        namespace: &str,
        run_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Value>> {
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        let values = metadata::read_page(
            &tx,
            "SELECT rowid FROM documents WHERE namespace=?1 AND (?2 IS NULL OR owner_run_id=?2) ORDER BY COALESCE(json_extract(payload,'$.created_at_us'),updated_at_us) DESC,id LIMIT ?3 OFFSET ?4",
            params![namespace, run_id, limit.min(501), offset],
            "SELECT payload FROM documents WHERE rowid=?1",
        )?;
        tx.commit()?;
        Ok(values)
    }

    pub fn documents_with_status(&self, namespace: &str, status: &str) -> Result<Vec<Value>> {
        let c = self.connection()?;
        let mut statement = c.prepare("SELECT payload FROM documents WHERE namespace=?1 AND json_extract(payload,'$.status')=?2 ORDER BY updated_at_us,id")?;
        statement
            .query_map(params![namespace, status], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }

    pub fn save_document(&self, namespace: &str, id: &str, value: &Value) -> Result<()> {
        write_document(&self.connection()?, namespace, id, value)
    }

    /// Persist a cross-client cancellation without overwriting a completed attempt.
    pub fn request_analysis_cancel(&self, id: &str) -> Result<Value> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let text: String = tx
            .query_row(
                "SELECT payload FROM documents WHERE namespace='analysis-results' AND id=?1",
                [id],
                |r| r.get(0),
            )
            .context("analyzer execution not found")?;
        let mut value: Value = serde_json::from_str(&text)?;
        let running = value["status"] == "running";
        if running {
            value["cancellation_requested"] = Value::Bool(true);
            write_document(&tx, "analysis-results", id, &value)?;
        }
        tx.commit()?;
        Ok(value)
    }
}

pub(super) fn write_document(
    connection: &Connection,
    namespace: &str,
    id: &str,
    value: &Value,
) -> Result<()> {
    let owner = if matches!(
        namespace,
        "analysis-results" | "cancellations" | "profile-inputs" | "call-count-requests"
    ) {
        Some(
            value["run_id"]
                .as_str()
                .context("analyzer result requires an owning run")?,
        )
    } else {
        None
    };
    connection.execute("INSERT INTO documents(namespace,id,payload,updated_at_us,owner_run_id) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(namespace,id) DO UPDATE SET payload=excluded.payload,updated_at_us=excluded.updated_at_us,owner_run_id=excluded.owner_run_id",params![namespace,id,serde_json::to_string(value)?,crate::model::now_us(),owner])?;
    Ok(())
}

//! Atomic result publication: selected inputs, cancellation, rows and metadata.
use super::{Store, documents::write_document};
use crate::{
    ingestion,
    model::relations::{Ownership, OwnershipKind, SelectedObservation},
    publication::{self, PublicationMetadata, PublicationRequest},
};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use std::collections::HashMap;

impl Store {
    /// Validate and publish a whole result set in one transaction. Replays are no-ops.
    pub fn accept(&self, run_id: &str, input_identity: &str, records: &[Value]) -> Result<bool> {
        self.publish_result(PublicationRequest {
            run_id,
            source_key: input_identity,
            input_identity,
            records,
            metadata: None,
            deadline: None,
        })
    }

    pub fn accept_import(
        &self,
        run_id: &str,
        source_key: &str,
        fingerprint: &str,
        records: &[Value],
    ) -> Result<bool> {
        self.publish_result(PublicationRequest {
            run_id,
            source_key,
            input_identity: &format!("normalization:{fingerprint}"),
            records,
            metadata: None,
            deadline: None,
        })
    }

    pub(crate) fn publish_result(&self, request: PublicationRequest<'_>) -> Result<bool> {
        use crate::model::identity::{ObservationId, ResultId};
        let PublicationRequest {
            run_id,
            source_key,
            input_identity,
            records,
            metadata,
            deadline,
        } = request;
        self.run(run_id)?;
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        publication::check_deadline(deadline)?;
        if let Some(id) =
            metadata.and_then(|value| value.analysis_execution_id().map(str::to_owned))
        {
            let cancelled: Option<bool> = tx.query_row("SELECT COALESCE(json_extract(payload,'$.cancellation_requested'),0) FROM documents WHERE namespace='analysis-results' AND id=?1",[id],|r|r.get(0)).optional()?;
            anyhow::ensure!(
                cancelled != Some(true),
                "analyzer execution was cancelled before publication"
            );
        }
        if let Some((id, active, previous_source)) = tx
            .query_row(
                "SELECT id,active,source_key FROM result_sets WHERE run_id=?1 AND input_identity=?2",
                params![run_id, input_identity],
                |r| Ok((ResultId::from(r.get::<_, String>(0)?), r.get::<_, bool>(1)?, r.get::<_, String>(2)?)),
            )
            .optional()?
        {
            anyhow::ensure!(previous_source == source_key, "input identity belongs to another result source");
            if !active {
                deactivate_source(&tx, run_id, source_key)?;
                let stale: i64 = tx.query_row("SELECT COUNT(*) FROM result_set_dependencies d JOIN result_sets r ON r.id=d.input_result_set_id WHERE d.result_set_id=?1 AND r.active=0", [id.as_str()], |r|r.get(0))?;
                anyhow::ensure!(stale == 0, "stored result depends on superseded inputs; recompute the analysis");
                tx.execute("UPDATE result_sets SET active=1 WHERE id=?1", [id.as_str()])?;
            }
            if let Some(metadata) = metadata {
                write_publication_metadata(&tx, metadata, &metadata.replayed_document(&id))?;
            }
            publication::check_deadline(deadline)?;
            tx.commit()?;
            return Ok(false);
        }
        deactivate_source(&tx, run_id, source_key)?;
        // The write transaction owns this selected snapshot through insertion.
        // Acceptance only needs relationship projections, never whole stack/source payloads.
        let selected: HashMap<ObservationId, SelectedObservation> = tx
            .prepare("SELECT o.id,o.kind,json_extract(o.record,'$.parent_id'),json_extract(o.record,'$.context_id'),o.result_set_id FROM observations o JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND r.active=1")?
            .query_map([run_id], |row| {
                let kind: String = row.get(1)?;
                Ok((ObservationId::from(row.get::<_, String>(0)?), SelectedObservation {
                    ownership: Ownership {
                        kind: OwnershipKind::from_wire(&kind),
                        parent_id: row.get::<_, Option<String>>(2)?.map(ObservationId::from),
                        context_id: row.get::<_, Option<String>>(3)?.map(ObservationId::from),
                    },
                    result_set_id: ResultId::from(row.get::<_, String>(4)?),
                }))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let existing = selected.keys().cloned().collect();
        ingestion::validate_ids(run_id, records, existing)?;
        let proposed = ingestion::links(records);
        let mut dependencies = publication::validate_relations(&proposed, &selected)?;
        if let Some(metadata) = metadata {
            dependencies.extend(metadata.declared_inputs()?);
        }
        for id in &dependencies {
            let selected: i64 = tx.query_row(
                "SELECT COUNT(*) FROM result_sets WHERE id=?1 AND run_id=?2 AND active=1",
                params![id.as_str(), run_id],
                |r| r.get(0),
            )?;
            anyhow::ensure!(
                selected == 1,
                "analysis input changed before publication; rerun with current selected results"
            );
        }
        let result_id = metadata
            .and_then(|value| value.analysis_execution_id().map(ResultId::from))
            .unwrap_or_else(|| ResultId::from(uuid::Uuid::new_v4().to_string()));
        tx.execute(
            "INSERT INTO result_sets(id,run_id,input_identity,source_key,active) VALUES(?1,?2,?3,?4,1)",
            params![result_id.as_str(), run_id, input_identity,source_key],
        )?;
        for id in dependencies {
            tx.execute("INSERT INTO result_set_dependencies(result_set_id,input_result_set_id) VALUES(?1,?2)",params![result_id.as_str(),id.as_str()])?;
        }
        {
            let mut insert=tx.prepare("INSERT INTO observations(run_id,id,kind,timestamp_us,result_set_id,record) VALUES(?1,?2,?3,?4,?5,?6)")?;
            for (r, observation) in records.iter().zip(&proposed) {
                insert.execute(params![
                    run_id,
                    observation.id.as_str(),
                    r["kind"].as_str(),
                    crate::model::numbers::unsigned_integer(&r["timestamp_us"]),
                    result_id.as_str(),
                    serde_json::to_string(r)?
                ])?;
            }
        }
        if let Some(metadata) = metadata {
            write_publication_metadata(&tx, metadata, metadata.document())?;
        }
        publication::check_deadline(deadline)?;
        tx.commit()?;
        Ok(true)
    }
}

fn write_publication_metadata(
    connection: &Connection,
    metadata: PublicationMetadata<'_>,
    document: &Value,
) -> Result<()> {
    let (namespace, id) = match metadata {
        PublicationMetadata::Analysis { execution_id, .. } => ("analysis-results", execution_id),
        PublicationMetadata::Normalization { input_id, .. } => ("profile-inputs", input_id),
    };
    write_document(connection, namespace, id, document)
}

fn deactivate_source(connection: &Connection, run_id: &str, source_key: &str) -> Result<()> {
    connection.execute("WITH RECURSIVE stale(id) AS (SELECT id FROM result_sets WHERE run_id=?1 AND source_key=?2 AND active=1 UNION SELECT d.result_set_id FROM result_set_dependencies d JOIN stale s ON d.input_result_set_id=s.id) UPDATE result_sets SET active=0 WHERE id IN(SELECT id FROM stale)",params![run_id,source_key])?;
    Ok(())
}

#[cfg(test)]
mod tests;

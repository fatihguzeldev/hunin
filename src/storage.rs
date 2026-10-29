use crate::{
    ingestion,
    model::{
        relations::{Ownership, OwnershipKind, SelectedObservation},
        runs::{Run, RunStatus},
    },
    publication::{self, PublicationMetadata, PublicationRequest},
};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

mod artifacts;
pub(crate) use artifacts::ArtifactByteLimit;
mod bootstrap;
pub use bootstrap::SCHEMA_VERSION;
mod inspection;
mod maintenance;
pub(crate) use maintenance::ActiveAnalysisRemoval;
mod observations;
pub use inspection::{
    InspectionArea, InspectionFinding, InspectionLimit, InspectionLimits, InspectionPathKind,
    InspectionScan, InspectionScanStatus, StorageInspection, WorkspaceMetadataReference,
};
pub(crate) use observations::RecordReadLimit;
pub use observations::RecordsSnapshot;
pub use observations::{
    QueryReadPort, QuerySnapshot, ReadBudgetExceeded, ReadDimension, ReadLimits, ReadUsage,
};

#[cfg(test)]
mod publication_tests;

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
}

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

    /// Page in SQL before decoding payloads; completed attempts keep creation order.
    pub fn documents_page(
        &self,
        namespace: &str,
        run_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Value>> {
        let c = self.connection()?;
        let mut statement = c.prepare("SELECT payload FROM documents WHERE namespace=?1 AND (?2 IS NULL OR owner_run_id=?2) ORDER BY COALESCE(json_extract(payload,'$.created_at_us'),updated_at_us) DESC,id LIMIT ?3 OFFSET ?4")?;
        statement
            .query_map(params![namespace, run_id, limit.min(501), offset], |r| {
                r.get::<_, String>(0)
            })?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
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
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn save_run(&self, run: &Run) -> Result<()> {
        self.connection()?.execute("INSERT INTO runs(id, started_at_us, manifest) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET manifest=excluded.manifest",params![run.id, run.started_at_us, serde_json::to_string(run)?])?;
        Ok(())
    }

    /// Merge import diagnostics without replacing execution finalization or
    /// recreating a recording that was removed while normalization was running.
    pub fn finish_recording_import(
        &self,
        run_id: &str,
        diagnostics: &[String],
        complete_new_run: bool,
    ) -> Result<Run> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let text: String = tx
            .query_row("SELECT manifest FROM runs WHERE id=?1", [run_id], |r| {
                r.get(0)
            })
            .with_context(|| format!("run removed before import completion: {run_id}"))?;
        let mut run: Run = serde_json::from_str(&text)?;
        anyhow::ensure!(
            run.status != RunStatus::Running,
            "cannot import into a running recording"
        );
        if complete_new_run {
            run.status = RunStatus::Completed;
        }
        run.diagnostics.extend_from_slice(diagnostics);
        tx.execute(
            "UPDATE runs SET manifest=?2 WHERE id=?1",
            params![run_id, serde_json::to_string(&run)?],
        )?;
        tx.commit()?;
        Ok(run)
    }

    pub fn run(&self, id: &str) -> Result<Run> {
        let text: String = self
            .connection()?
            .query_row("SELECT manifest FROM runs WHERE id=?1", [id], |r| r.get(0))
            .with_context(|| format!("run not found: {id}"))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn runs(&self, limit: usize, offset: usize) -> Result<Vec<Run>> {
        let c = self.connection()?;
        let mut statement = c.prepare(
            "SELECT manifest FROM runs ORDER BY started_at_us DESC,id LIMIT ?1 OFFSET ?2",
        )?;
        let rows =
            statement.query_map(params![limit.min(500), offset], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

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

    pub fn publish(
        &self,
        run_id: &str,
        source_key: &str,
        input_identity: &str,
        records: &[Value],
        document: Option<(&str, &str, &Value)>,
    ) -> Result<bool> {
        // Compatibility adapter for existing callers of the generic API. New
        // application code supplies lifecycle metadata explicitly.
        let metadata = document.map(|(namespace, id, document)| match namespace {
            "analysis-results" => PublicationMetadata::Analysis {
                execution_id: id,
                document,
            },
            "profile-inputs" => PublicationMetadata::Normalization {
                input_id: id,
                document,
            },
            _ => PublicationMetadata::Other {
                namespace,
                id,
                document,
            },
        });
        self.publish_result(PublicationRequest {
            run_id,
            source_key,
            input_identity,
            records,
            metadata,
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
                    r["timestamp_us"].as_u64(),
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
        PublicationMetadata::Other { namespace, id, .. } => (namespace, id),
    };
    write_document(connection, namespace, id, document)
}

fn write_document(connection: &Connection, namespace: &str, id: &str, value: &Value) -> Result<()> {
    let owner = if matches!(
        namespace,
        "analysis-results" | "cancellations" | "profile-inputs"
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

fn deactivate_source(connection: &Connection, run_id: &str, source_key: &str) -> Result<()> {
    connection.execute("WITH RECURSIVE stale(id) AS (SELECT id FROM result_sets WHERE run_id=?1 AND source_key=?2 AND active=1 UNION SELECT d.result_set_id FROM result_set_dependencies d JOIN stale s ON d.input_result_set_id=s.id) UPDATE result_sets SET active=0 WHERE id IN(SELECT id FROM stale)",params![run_id,source_key])?;
    Ok(())
}

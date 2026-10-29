mod capabilities;
mod comparison;
mod contracts;
mod errors;
mod inputs;
mod inspection;
pub(crate) use errors::operation_failure;

use crate::{
    execution::{Engine, StartRequest},
    model::{now_us, runs::Run},
    query,
    storage::Store,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::OnceLock,
};

#[derive(Clone)]
pub struct Operations {
    pub engine: Engine,
    pub analyses: crate::analysis::Engine,
}

pub fn catalog() -> &'static Vec<Value> {
    static CATALOG: OnceLock<Vec<Value>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut operations: Vec<Value> =
            serde_json::from_str(include_str!("../../protocol/operations.json"))
                .expect("embedded operation catalog");
        let responses: Value =
            serde_json::from_str(include_str!("../../protocol/response.schema.json"))
                .expect("embedded response definitions");
        let definitions = responses["$defs"]
            .as_object()
            .expect("response definitions");
        for operation in &mut operations {
            if let Some(schema) = operation.get_mut("outputSchema") {
                *schema = contracts::bundle(schema, definitions).expect("embedded output schema");
            }
        }
        operations
    })
}

impl Operations {
    pub fn new(store: Store) -> Self {
        Self {
            engine: Engine::new(store.clone()),
            analyses: crate::analysis::Engine::new(store),
        }
    }
    pub async fn shutdown(&self) -> Result<()> {
        let (recordings, analyses) = tokio::join!(self.engine.shutdown(), self.analyses.shutdown());
        recordings?;
        analyses?;
        Ok(())
    }
    pub async fn call(&self, name: &str, mut params: Value) -> Result<Value> {
        let definition = catalog()
            .iter()
            .find(|s| s["name"] == name)
            .ok_or_else(errors::Failure::unknown_operation)?;
        jsonschema::validator_for(&definition["inputSchema"])?
            .validate(&params)
            .map_err(|error| {
                errors::Failure::invalid_parameters(format!(
                    "invalid operation parameters: {error}"
                ))
            })?;
        inputs::canonicalize_integers(&definition["inputSchema"], &mut params);
        let store = self.engine.store();
        let id = || params["run_id"].as_str().context("run_id required");
        let limit = params["limit"].as_u64().unwrap_or(100) as usize;
        let cursor = params["cursor"]
            .as_str()
            .map(str::parse::<usize>)
            .transpose()
            .map_err(errors::Failure::invalid_parameters)?
            .unwrap_or(0);
        match name {
            "storage.inspect" => {
                let store = store.clone();
                tokio::task::spawn_blocking(move || inspection::inspect(&store, &params))
                    .await
                    .context("storage inspection task failed")?
            }
            "capabilities" => tokio::task::spawn_blocking(capabilities::discover)
                .await
                .context("capability discovery task failed"),
            "runs.list" => {
                let mut runs = store.runs(limit + 1, cursor)?;
                let more = runs.len() > limit;
                runs.truncate(limit);
                Ok(json!({"runs":runs,"next_cursor":more.then(||(cursor+limit).to_string())}))
            }
            "runs.get" => {
                let run = store.run(id()?)?;
                let snapshot = store.records_snapshot(&run.id)?;
                Ok(
                    json!({"run":run,"summary":query::summary(&snapshot.records),"result_identity":snapshot.result_identity}),
                )
            }
            "runs.start" => Ok(
                json!({"run":self.engine.start(serde_json::from_value::<StartRequest>(params).map_err(errors::Failure::invalid_parameters)?).await?}),
            ),
            "runs.cancel" => Ok(json!({"run":self.engine.cancel(id()?).await?})),
            "runs.remove" => {
                if store
                    .documents("analysis-results")?
                    .iter()
                    .any(|r| r["run_id"] == id().unwrap() && r["status"] == "running")
                {
                    return Err(errors::Failure::conflict(
                        "cancel active analyzer executions before removing this recording",
                    )
                    .into());
                }
                store.remove_run(id()?)?;
                Ok(json!({"removed":true}))
            }
            "profiles.hotspots" => Ok(query::hotspots(
                &store.records(id()?)?,
                params["weight_unit"].as_str().unwrap_or(""),
                limit,
            )),
            "profiles.tree" => Ok(query::tree_page(&store.records(id()?)?, &params)),
            "timeline.query" => Ok(query::timeline(&store.records(id()?)?, &params)),
            "resources.query" => Ok(query::resources_page(&store.records(id()?)?, &params)),
            "contexts.query" => {
                let mut selected = store.context_records_page(id()?, limit + 1, cursor)?;
                let more = selected.len() > limit;
                selected.truncate(limit);
                Ok(
                    json!({"contexts":selected.iter().filter(|r|r["kind"]=="context").collect::<Vec<_>>(),"relations":selected.iter().filter(|r|r["kind"]=="relation").collect::<Vec<_>>(),"next_cursor":more.then(||(cursor+limit).to_string())}),
                )
            }
            "evidence.get" => {
                let wanted = serde_json::from_value::<Vec<String>>(params["ids"].clone())?;
                Ok(json!({"records":store.evidence_records(id()?, &wanted)?}))
            }
            "artifacts.list" => Ok(metadata_page(
                "artifacts",
                store.artifacts_page(id()?, limit + 1, cursor)?,
                limit,
                cursor,
            )),
            "artifacts.read" => {
                let path = store.artifact_path(id()?, params["artifact_id"].as_str().unwrap())?;
                let mut file = fs::File::open(path)?;
                let size = file.metadata()?.len();
                let start = params["offset"].as_u64().unwrap_or(0);
                if start > size {
                    bail!("artifact offset outside content");
                }
                file.seek(SeekFrom::Start(start))?;
                let mut bytes = Vec::new();
                file.take(limit.min(5000) as u64).read_to_end(&mut bytes)?;
                let end = start + bytes.len() as u64;
                Ok(
                    json!({"content":String::from_utf8_lossy(&bytes),"encoding":"utf8_lossy","next_offset":(end<size).then_some(end),"size":size}),
                )
            }
            "source.read" => crate::source::read(store, &params),
            "recordings.import" => {
                let operations = self.clone();
                tokio::task::spawn_blocking(move || operations.import(&params))
                    .await
                    .context("recording import task failed")?
            }
            "recordings.reanalyze" => {
                let store = store.clone();
                let run_id = id()?.to_owned();
                let artifact_id = params["artifact_id"].as_str().unwrap().to_owned();
                tokio::task::spawn_blocking(move || {
                    let publication = crate::recordings::reanalyze(&store, &run_id, &artifact_id)?;
                    Ok(json!({"accepted":publication.accepted,"diagnostics":publication.diagnostics,"summary":query::summary(&store.records(&run_id)?)}))
                }).await.context("recording reanalysis task failed")?
            }
            "comparisons.create" => comparison::compare(
                store,
                params["before"].as_str().unwrap(),
                params["after"].as_str().unwrap(),
                params["weight_unit"].as_str(),
            ),
            "scenarios.list" => Ok(metadata_page(
                "scenarios",
                store.documents_page("scenarios", None, limit + 1, cursor)?,
                limit,
                cursor,
            )),
            "scenarios.save" => {
                let mut scenario = params;
                let id = scenario["id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                scenario["id"] = json!(id);
                store.save_document("scenarios", &id, &scenario)?;
                Ok(json!({"scenario":scenario}))
            }
            "scenarios.run" => {
                let mut scenario =
                    store.document("scenarios", params["scenario_id"].as_str().unwrap())?;
                scenario.as_object_mut().unwrap().remove("id");
                if let Some(name) = params.get("name") {
                    scenario["name"] = name.clone();
                }
                let request = serde_json::from_value(scenario)?;
                Ok(json!({"run":self.engine.start(request).await?}))
            }
            "analyses.list" => Ok(metadata_page(
                "analyses",
                store.documents_page("analyses", None, limit + 1, cursor)?,
                limit,
                cursor,
            )),
            "analyses.results" => {
                let operations = self.clone();
                tokio::task::spawn_blocking(move || {
                    operations.analysis_results(&params, limit, cursor)
                })
                .await
                .context("analysis result read task failed")?
            }
            "analyses.register" => {
                let analyses = self.analyses.clone();
                tokio::task::spawn_blocking(move || analyses.register(&params))
                    .await
                    .context("analysis registration task failed")?
            }
            "analyses.run" => self.analyses.run(&params).await,
            "analyses.cancel" => {
                self.analyses
                    .cancel(params["execution_id"].as_str().unwrap())
                    .await
            }
            _ => bail!("unknown operation"),
        }
    }

    fn analysis_results(&self, params: &Value, limit: usize, cursor: usize) -> Result<Value> {
        self.analyses.recover()?;
        let store = self.engine.store();
        let run_id = params["run_id"].as_str().context("run_id required")?;
        store.run(run_id)?;
        let mut results = if let Some(execution_id) = params["execution_id"].as_str() {
            if cursor == 0 {
                store
                    .document_for_run("analysis-results", execution_id, run_id)?
                    .into_iter()
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            store.documents_page("analysis-results", Some(run_id), limit + 1, cursor)?
        };
        for row in &mut results {
            let selected = store.selected_result_id(
                run_id,
                &format!("analysis:{}", row["analysis_id"].as_str().unwrap_or("")),
            )?;
            let result_id = row["result_set_id"].as_str().or_else(|| row["id"].as_str());
            row["selected"] = json!(result_id.is_some() && selected.as_deref() == result_id);
        }
        Ok(metadata_page("results", results, limit, cursor))
    }

    fn import(&self, params: &Value) -> Result<Value> {
        let store = self.engine.store();
        let path = PathBuf::from(params["path"].as_str().unwrap()).canonicalize()?;
        anyhow::ensure!(path.is_file(), "recording import requires a regular file");
        let format = params["format"].as_str().unwrap();
        let run = if let Some(id) = params["run_id"].as_str() {
            let run = store.run(id)?;
            anyhow::ensure!(
                run.status != "running",
                "cannot import into a running recording"
            );
            run
        } else {
            let run=Run{id:uuid::Uuid::new_v4().to_string(),name:params["name"].as_str().unwrap_or("Imported recording").into(),status:"failed".into(),command:vec![],cwd:path.parent().unwrap().display().to_string(),runtime:"imported".into(),started_at_us:now_us(),finished_at_us:Some(now_us()),exit_code:None,diagnostics:vec!["Imported data retains its original collection limits; execution correctness is unknown".into()],source:json!({"fingerprint":"unknown","build_source_verified":false}),correctness:json!({"passed":null,"reason":"imported"})};
            store.save_run(&run)?;
            run
        };
        let imported = (|| {
            store.add_artifact(&run.id, &path, format)?;
            let profile = crate::importers::import(&path, format, &run.id)?;
            crate::recordings::publish(store, &run.id, profile)
        })();
        let publication = match imported {
            Ok(publication) => publication,
            Err(error) => {
                if params["run_id"].is_null() {
                    let diagnostic =
                        format!("Import failed; any preserved inputs remain available: {error:#}");
                    store.finish_recording_import(&run.id, &[diagnostic], false)?;
                }
                return Err(error);
            }
        };
        let run = store.finish_recording_import(
            &run.id,
            &publication.diagnostics,
            params["run_id"].is_null(),
        )?;
        Ok(
            json!({"run":run,"accepted":publication.accepted,"summary":query::summary(&store.records(&run.id)?)}),
        )
    }
}

fn metadata_page<T: serde::Serialize>(
    key: &str,
    mut rows: Vec<T>,
    limit: usize,
    cursor: usize,
) -> Value {
    let more = rows.len() > limit;
    rows.truncate(limit);
    let mut result = json!({"next_cursor":more.then(||(cursor + limit).to_string())});
    result[key] = json!(rows);
    result
}

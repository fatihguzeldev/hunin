mod capabilities;
mod comparison;
mod contracts;
mod errors;
mod inputs;
mod inspection;
mod reads;
mod responses;
mod workers;
pub(crate) use errors::operation_failure;

use crate::{
    execution::{Engine, StartRequest},
    model::{now_us, runs::Run},
    query,
    storage::Store,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::OnceLock};

#[derive(Clone)]
pub struct Operations {
    pub engine: Engine,
    pub analyses: crate::analysis::Engine,
    aggregate_work: workers::WorkPool,
    metadata_work: workers::WorkPool,
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
            aggregate_work: workers::WorkPool::new("operations_family_aggregation", 2),
            metadata_work: workers::WorkPool::new("operations_family_metadata", 8),
        }
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.aggregate_work.close();
        self.metadata_work.close();
        let (recordings, analyses) = tokio::join!(self.engine.shutdown(), self.analyses.shutdown());
        tokio::join!(self.aggregate_work.drain(), self.metadata_work.drain());
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
        let read_response = definition["_meta"]["hunin/lifecycle"]["kind"] == "read";
        let result: Result<Value> = match name {
            "storage.inspect" => {
                let store = store.clone();
                self.aggregate_work
                    .read(move |_| inspection::inspect(&store, &params))
                    .await
            }
            "capabilities" => {
                self.aggregate_work
                    .read(|_| Ok(capabilities::discover()))
                    .await
            }
            "runs.list" | "runs.metadata" | "contexts.query" | "evidence.get"
            | "artifacts.list" | "artifacts.read" | "source.read" | "scenarios.list"
            | "analyses.list" => {
                let store = store.clone();
                let name = name.to_owned();
                self.metadata_work
                    .read(move |interest| {
                        reads::query(&store, &name, &params, limit, cursor, &interest)
                    })
                    .await
            }
            "runs.get" => {
                let store = store.clone();
                let run_id = id()?.to_owned();
                self.aggregate_work.read(move |interest| {
                    let run = store.run(&run_id)?;
                    let snapshot = query::read_recording(&store, &run_id, query::RecordingQuery::Summary, Default::default(), move || interest.check())?;
                    Ok(json!({"run":run,"summary":snapshot.value,"result_identity":snapshot.result_identity}))
                }).await
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
            "profiles.hotspots" | "profiles.tree" | "timeline.query" | "resources.query" => {
                let store = store.clone();
                let run_id = id()?.to_owned();
                let name = name.to_owned();
                self.aggregate_work
                    .read(move |interest| {
                        let request = match name.as_str() {
                            "profiles.hotspots" => query::RecordingQuery::Hotspots {
                                unit: params["weight_unit"].as_str().unwrap_or(""),
                                limit,
                            },
                            "profiles.tree" => query::RecordingQuery::Tree(&params),
                            "timeline.query" => query::RecordingQuery::Timeline(&params),
                            "resources.query" => query::RecordingQuery::Resources(&params),
                            _ => unreachable!(),
                        };
                        Ok(query::read_recording(
                            &store,
                            &run_id,
                            request,
                            Default::default(),
                            move || interest.check(),
                        )?
                        .value)
                    })
                    .await
            }
            "recordings.import" => {
                let operations = self.clone();
                self.aggregate_work
                    .run(move || operations.import(&params))
                    .await
            }
            "recordings.reanalyze" => {
                let store = store.clone();
                let run_id = id()?.to_owned();
                let artifact_id = params["artifact_id"].as_str().unwrap().to_owned();
                self.aggregate_work.run(move || {
                    let publication = crate::recordings::reanalyze(&store, &run_id, &artifact_id)?;
                    Ok(json!({"accepted":publication.accepted,"diagnostics":publication.diagnostics,"summary":query::summary(&store.records(&run_id)?)}))
                }).await
            }
            "comparisons.create" => {
                let store = store.clone();
                self.aggregate_work
                    .read(move |interest| {
                        comparison::compare(
                            &store,
                            params["before"].as_str().unwrap(),
                            params["after"].as_str().unwrap(),
                            params["weight_unit"].as_str(),
                            move || interest.check(),
                        )
                    })
                    .await
            }
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
            "analyses.results" => {
                let operations = self.clone();
                self.metadata_work
                    .read(move |_| operations.analysis_results(&params, limit, cursor))
                    .await
            }
            "analyses.register" => {
                let analyses = self.analyses.clone();
                self.aggregate_work
                    .run(move || analyses.register(&params))
                    .await
            }
            "analyses.run" => self.analyses.run(&params).await,
            "analyses.cancel" => {
                self.analyses
                    .cancel(params["execution_id"].as_str().unwrap())
                    .await
            }
            _ => bail!("unknown operation"),
        };
        let result = result?;
        if read_response {
            responses::check(&result)?;
        }
        Ok(result)
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

mod analyses;
mod calls;
mod capabilities;
mod comparison;
mod contracts;
mod errors;
mod inputs;
mod inspection;
mod reads;
mod recordings;
mod responses;
mod workers;
pub(crate) use errors::operation_failure;

use crate::{
    execution::{Engine, StartRequest},
    query,
    storage::Store,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::sync::OnceLock;

/// Project a typed operation failure without changing a transport's default output.
pub fn describe_failure(operation: &str, error: &anyhow::Error) -> Value {
    errors::operation_failure(operation, error)
}

/// Transport-owned parameter decoding shares the operation validation origin.
pub fn invalid_parameters_error(message: impl std::fmt::Display) -> anyhow::Error {
    errors::Failure::invalid_parameters(message).into()
}

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
        Self::from_engine(Engine::new(store))
    }
    /// Return startup recovery failures to machine transports instead of logging.
    pub fn try_new(store: Store) -> Result<Self> {
        Engine::try_new(store).map(Self::from_engine)
    }
    fn from_engine(engine: Engine) -> Self {
        let store = engine.store().clone();
        Self {
            engine,
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
            "calls.plan" | "calls.measure" => {
                let planning_store = store.clone();
                let planning_params = params.clone();
                let prepared = self
                    .aggregate_work
                    .read(move |_| calls::plan(&planning_store, &planning_params))
                    .await?;
                if name == "calls.plan" {
                    Ok(json!({"plan":prepared.2}))
                } else {
                    let engine = self.engine.clone();
                    tokio::spawn(async move { calls::measure(engine, params, prepared).await })
                        .await?
                }
            }
            "calls.query" => {
                let store = store.clone();
                self.aggregate_work
                    .read(move |interest| calls::query(&store, &params, move || interest.check()))
                    .await
            }
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
            "runs.list" | "runs.metadata" | "runs.removal" | "contexts.query" | "evidence.get"
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
                let store = store.clone();
                let run_id = id()?.to_owned();
                self.aggregate_work
                    .run(move || {
                        store.remove_run(&run_id)?;
                        Ok(json!({"removed":true}))
                    })
                    .await
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
                self.aggregate_work
                    .run(move || {
                        let deadline =
                            Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
                        let publication = crate::recordings::reanalyze_controlled(
                            &store,
                            &run_id,
                            &artifact_id,
                            deadline,
                        )?;
                        let mut result = reads::summary_after_mutation(&store, &run_id, deadline);
                        result["accepted"] = json!(publication.accepted);
                        result["diagnostics"] = json!(publication.diagnostics);
                        Ok(result)
                    })
                    .await
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
                let store = store.clone();
                self.metadata_work
                    .run(move || {
                        store.save_document("scenarios", &id, &scenario)?;
                        Ok(json!({"scenario":scenario}))
                    })
                    .await
            }
            "scenarios.run" => {
                let store = store.clone();
                let scenario_id = params["scenario_id"].as_str().unwrap().to_owned();
                let mut scenario = self
                    .metadata_work
                    .read(move |_| store.document("scenarios", &scenario_id))
                    .await?;
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

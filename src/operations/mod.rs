mod capabilities;
mod comparison;

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
        serde_json::from_str(include_str!("../../protocol/operations.json"))
            .expect("embedded operation catalog")
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
    pub async fn call(&self, name: &str, params: Value) -> Result<Value> {
        let definition = catalog()
            .iter()
            .find(|s| s["name"] == name)
            .context("unknown operation")?;
        jsonschema::validator_for(&definition["inputSchema"])?
            .validate(&params)
            .map_err(|e| anyhow::anyhow!("invalid operation parameters: {e}"))?;
        let store = self.engine.store();
        let id = || params["run_id"].as_str().context("run_id required");
        let limit = params["limit"].as_u64().unwrap_or(100) as usize;
        let cursor = params["cursor"]
            .as_str()
            .map(str::parse::<usize>)
            .transpose()?
            .unwrap_or(0);
        match name {
            "capabilities" => Ok(capabilities::discover()),
            "runs.list" => {
                let mut runs = store.runs(limit + 1, cursor)?;
                let more = runs.len() > limit;
                runs.truncate(limit);
                Ok(json!({"runs":runs,"next_cursor":more.then(||(cursor+limit).to_string())}))
            }
            "runs.get" => {
                let run = store.run(id()?)?;
                let records = store.records(&run.id)?;
                Ok(json!({"run":run,"summary":query::summary(&records)}))
            }
            "runs.start" => Ok(
                json!({"run":self.engine.start(serde_json::from_value::<StartRequest>(params)?).await?}),
            ),
            "runs.cancel" => Ok(json!({"run":self.engine.cancel(id()?).await?})),
            "runs.remove" => {
                if store
                    .documents("analysis-results")?
                    .iter()
                    .any(|r| r["run_id"] == id().unwrap() && r["status"] == "running")
                {
                    bail!("cancel active analyzer executions before removing this recording");
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
                let records = store.records(id()?)?;
                let rows = records
                    .iter()
                    .filter(|r| r["kind"] == "context" || r["kind"] == "relation")
                    .collect::<Vec<_>>();
                let selected = rows
                    .iter()
                    .skip(cursor)
                    .take(limit)
                    .copied()
                    .collect::<Vec<_>>();
                Ok(
                    json!({"contexts":selected.iter().filter(|r|r["kind"]=="context").collect::<Vec<_>>(),"relations":selected.iter().filter(|r|r["kind"]=="relation").collect::<Vec<_>>(),"next_cursor":(cursor+limit<rows.len()).then(||(cursor+limit).to_string())}),
                )
            }
            "evidence.get" => {
                let records = store.records(id()?)?;
                let wanted = params["ids"].as_array().unwrap();
                Ok(
                    json!({"records":records.iter().filter(|r|wanted.contains(&r["id"])).collect::<Vec<_>>()}),
                )
            }
            "artifacts.list" => Ok(json!({"artifacts":store.artifacts(id()?)?})),
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
            "recordings.import" => self.import(&params),
            "comparisons.create" => comparison::compare(
                store,
                params["before"].as_str().unwrap(),
                params["after"].as_str().unwrap(),
            ),
            "scenarios.list" => Ok(json!({"scenarios":store.documents("scenarios")?})),
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
            "analyses.list" => Ok(json!({"analyses":store.documents("analyses")?})),
            "analyses.results" => {
                self.analyses.recover()?;
                let run_id = id()?;
                let mut results = store
                    .documents("analysis-results")?
                    .into_iter()
                    .filter(|r| r["run_id"] == run_id)
                    .collect::<Vec<_>>();
                for row in &mut results {
                    let selected = store.selected_result_id(
                        run_id,
                        &format!("analysis:{}", row["analysis_id"].as_str().unwrap_or("")),
                    )?;
                    let result_id = row["result_set_id"].as_str().or_else(|| row["id"].as_str());
                    row["selected"] = json!(selected.as_deref() == result_id);
                }
                Ok(json!({"results":results}))
            }
            "analyses.register" => self.analyses.register(&params),
            "analyses.run" => self.analyses.run(&params).await,
            "analyses.cancel" => {
                self.analyses
                    .cancel(params["execution_id"].as_str().unwrap())
                    .await
            }
            _ => bail!("unknown operation"),
        }
    }

    fn import(&self, params: &Value) -> Result<Value> {
        let store = self.engine.store();
        let path = PathBuf::from(params["path"].as_str().unwrap()).canonicalize()?;
        let format = params["format"].as_str().unwrap();
        let run = if let Some(id) = params["run_id"].as_str() {
            store.run(id)?
        } else {
            let run=Run{id:uuid::Uuid::new_v4().to_string(),name:params["name"].as_str().unwrap_or("Imported recording").into(),status:"completed".into(),command:vec![],cwd:path.parent().unwrap().display().to_string(),runtime:"imported".into(),started_at_us:now_us(),finished_at_us:Some(now_us()),exit_code:None,diagnostics:vec!["Imported data retains its original collection limits; execution correctness is unknown".into()],source:json!({"fingerprint":"unknown","build_source_verified":false}),correctness:json!({"passed":null,"reason":"imported"})};
            store.save_run(&run)?;
            run
        };
        let profile = crate::importers::import(&path, format, &run.id)?;
        let raw = store.add_artifact(&run.id, &path, format)?;
        for sidecar in &profile.sidecars {
            store.add_artifact(&run.id, sidecar, "profile-metadata")?;
        }
        let accepted = store.accept_import(
            &run.id,
            &format!("import-v1:{}", raw.sha256),
            &profile.input_fingerprint,
            &profile.records,
        )?;
        let mut run = run;
        run.diagnostics.extend(profile.diagnostics);
        store.save_run(&run)?;
        Ok(
            json!({"run":run,"accepted":accepted,"summary":query::summary(&store.records(&run.id)?)}),
        )
    }
}

use super::{
    dependencies,
    input::{self, Input},
    package, process,
};
use crate::{model::now_us, storage::Store};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Mutex, watch},
    time::Instant,
};

#[derive(Clone)]
pub struct Engine {
    pub(super) store: Store,
    pub(super) closing: Arc<AtomicBool>,
    pub(super) owner: crate::execution::ownership::Owner,
    pub(super) active: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
}
impl Engine {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            closing: Arc::new(AtomicBool::new(false)),
            owner: crate::execution::ownership::Owner::current(),
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    pub fn register(&self, params: &Value) -> Result<Value> {
        package::register(&self.store, params)
    }
    pub async fn run(&self, params: &Value) -> Result<Value> {
        if self.closing.load(Ordering::Acquire) {
            bail!("analysis service is shutting down");
        }
        self.recover()?;
        let run_id = params["run_id"]
            .as_str()
            .context("analysis run_id required")?;
        let analysis_id = params["analysis_id"]
            .as_str()
            .context("analysis_id required")?;
        let timeout = params["timeout_seconds"].as_f64().unwrap_or(60.0);
        if !timeout.is_finite() || timeout <= 0.0 || timeout > 86400.0 {
            bail!("analysis timeout_seconds must be positive and at most 86400");
        }
        let attempt_started_at_us = now_us();
        let deadline = Instant::now() + Duration::from_secs_f64(timeout);
        let analysis = self.store.document("analyses", analysis_id)?;
        let phase = Instant::now();
        package::verify(&analysis)?;
        let mut timings =
            json!({"initial_package_verification":phase.elapsed().as_micros() as u64});
        process::check_deadline(deadline)?;
        let phase = Instant::now();
        let input = input::snapshot(&self.store, &analysis, run_id, || {
            process::check_deadline(deadline)
        })?;
        timings["input_snapshot"] = json!(phase.elapsed().as_micros() as u64);
        process::check_deadline(deadline)?;
        let mut active = self.active.lock().await;
        if self.closing.load(Ordering::Acquire) {
            bail!("analysis service is shutting down");
        }
        process::check_deadline(deadline)?;
        let id = uuid::Uuid::new_v4().to_string();
        let directory = self.store.root().join("analysis-executions").join(&id);
        let phase = Instant::now();
        fs::create_dir_all(&directory)?;
        fs::write(directory.join("input.json"), &input.bytes)?;
        timings["input_staging"] = json!(phase.elapsed().as_micros() as u64);
        let document = json!({"id":id,"result_set_id":id,"analysis_id":analysis_id,"run_id":run_id,"version":analysis["version"],"package_digest":analysis["package_digest"],"status":"running","created_at_us":now_us(),"attempt_started_at_us":attempt_started_at_us,"timeout_seconds":timeout,"deadline_us":attempt_started_at_us.saturating_add((timeout*1_000_000.0)as u64),"budget_scope":"preparation_setup_execution_verification_retention_publication_checkpoints","phase_timings_us":timings,"owner":self.owner,"input_result_sets":input.result_ids,"input_fingerprint":input.fingerprint,"diagnostics":[],"execution_dir":directory});
        self.store
            .save_document("analysis-results", &id, &document)?;
        let (sender, cancellation) = watch::channel(false);
        active.insert(id.clone(), sender.clone());
        drop(active);
        let monitor = self.clone();
        let monitor_id = id.clone();
        tokio::spawn(async move {
            loop {
                match monitor.store.document("analysis-results", &monitor_id) {
                    Ok(row) if row["status"] == "running" => {
                        if row["cancellation_requested"] == true {
                            let _ = sender.send(true);
                            break;
                        }
                    }
                    _ => break,
                }
                if !monitor.active.lock().await.contains_key(&monitor_id) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        let engine = self.clone();
        let execution_id = id.clone();
        tokio::spawn(async move {
            engine
                .complete(analysis, input, document, directory, cancellation, deadline)
                .await;
            engine.active.lock().await.remove(&execution_id);
        });
        Ok(json!({"result_set_id":id,"status":"running","diagnostics":[]}))
    }
    async fn analyze(
        &self,
        analysis: &Value,
        input: &Input,
        directory: &Path,
        cancellation: watch::Receiver<bool>,
        deadline: Instant,
        timings: &mut Value,
    ) -> Result<Vec<Value>> {
        let control = process::Control::new(cancellation, deadline);
        control.check()?;
        let phase = Instant::now();
        let source = package::verify(analysis);
        timings["package_verification"] = json!(phase.elapsed().as_micros() as u64);
        let source = source?;
        control.check()?;
        let phase = Instant::now();
        let workspace = directory.join("workspace");
        fs::create_dir_all(&workspace)?;
        for (path, identity) in analysis["files"]
            .as_object()
            .context("package file manifest missing")?
        {
            control.check()?;
            let bytes = fs::read(source.join(path))?;
            if package::digest(&bytes) != identity["sha256"] {
                bail!("managed package changed while preparing execution");
            }
            let destination = workspace.join(path);
            fs::create_dir_all(destination.parent().unwrap())?;
            fs::write(&destination, bytes)?;
            fs::set_permissions(destination, fs::metadata(source.join(path))?.permissions())?;
        }
        timings["workspace_copy"] = json!(phase.elapsed().as_micros() as u64);
        control.check()?;
        let phase = Instant::now();
        let prepared =
            dependencies::prepare(analysis, &workspace, directory, control.clone()).await;
        timings["dependency_setup"] = json!(phase.elapsed().as_micros() as u64);
        let (command, metadata) = prepared?;
        control.check()?;
        fs::write(
            directory.join("dependencies.json"),
            serde_json::to_vec_pretty(&metadata)?,
        )?;
        let phase = Instant::now();
        let status = process::execute(
            &command,
            &workspace,
            Some(&directory.join("input.json")),
            &directory.join("stdout.jsonl"),
            &directory.join("stderr.txt"),
            control.clone(),
        )
        .await;
        timings["subprocess"] = json!(phase.elapsed().as_micros() as u64);
        let status = status?;
        control.check()?;
        if !status.success() {
            bail!(
                "analyzer exited unsuccessfully ({:?}); inspect its retained stdout/stderr",
                status.code()
            );
        }
        let phase = Instant::now();
        let text = fs::read_to_string(directory.join("stdout.jsonl"))
            .context("analyzer output must be UTF-8 JSONL")?;
        let inputs = &input.record_ids;
        let mut records = Vec::new();
        for (line, value) in text
            .lines()
            .enumerate()
            .filter(|(_, text)| !text.trim().is_empty())
        {
            control.check()?;
            let mut record: Value = serde_json::from_str(value)
                .with_context(|| format!("invalid analyzer JSONL at line {}", line + 1))?;
            if !record.is_object() || record["basis"] != "derived" {
                bail!(
                    "analyzer observations must retain derived basis (line {})",
                    line + 1
                );
            }
            let evidence = record["evidence"]
                .as_array()
                .filter(|v| !v.is_empty())
                .context("every derived observation needs input record evidence")?;
            for reference in evidence {
                if !reference.as_str().is_some_and(|id| inputs.contains(id)) {
                    bail!("derived evidence must reference an input observation from this run");
                }
            }
            if record.get("attributes").is_none() {
                record["attributes"] = json!({});
            }
            if !record["attributes"].is_object() {
                bail!("analyzer attributes must be an object");
            }
            record["attributes"]["hunin_analysis_id"] = analysis["id"].clone();
            record["attributes"]["analysis_package_digest"] = analysis["package_digest"].clone();
            records.push(record);
        }
        timings["output_validation"] = json!(phase.elapsed().as_micros() as u64);
        control.check()?;
        let phase = Instant::now();
        let current = input::snapshot(&self.store, analysis, &input.run_id, || control.check());
        timings["input_recheck"] = json!(phase.elapsed().as_micros() as u64);
        if current?.fingerprint != input.fingerprint {
            bail!("analysis inputs changed during execution; no stale output was published");
        }
        control.check()?;
        let phase = Instant::now();
        let verified = package::verify(analysis);
        timings["final_package_verification"] = json!(phase.elapsed().as_micros() as u64);
        verified?;
        control.check()?;
        Ok(records)
    }
    async fn complete(
        &self,
        analysis: Value,
        input: Input,
        mut document: Value,
        directory: PathBuf,
        cancellation: watch::Receiver<bool>,
        deadline: Instant,
    ) {
        let mut timings = document["phase_timings_us"].clone();
        let result = self
            .analyze(
                &analysis,
                &input,
                &directory,
                cancellation.clone(),
                deadline,
                &mut timings,
            )
            .await;
        document["phase_timings_us"] = timings;
        let id = document["id"].as_str().unwrap().to_owned();
        let requested = self
            .store
            .document("analysis-results", &id)
            .is_ok_and(|row| row["cancellation_requested"] == true);
        document["cancellation_requested"] = json!(requested);
        document["finished_at_us"] = json!(now_us());
        document["status"] = json!(if requested || *cancellation.borrow() {
            "cancelled"
        } else if result.is_ok() {
            "accepted"
        } else if Instant::now() >= deadline {
            "timed_out"
        } else {
            "failed"
        });
        if let Err(error) = &result {
            document["diagnostics"] = json!([format!("{error:#}")]);
        }
        let run_id = document["run_id"].as_str().unwrap().to_owned();
        let id = document["id"].as_str().unwrap().to_owned();
        let retention = Instant::now();
        if let Err(error) = self.retain(&mut document, &directory) {
            document["status"] = json!("failed");
            document["finished_at_us"] = json!(now_us());
            document["diagnostics"] =
                json!([format!("failed to retain analysis evidence: {error:#}")]);
            let _ = self.store.save_document("analysis-results", &id, &document);
            return;
        }
        document["phase_timings_us"]["retention"] = json!(retention.elapsed().as_micros() as u64);
        document["finished_at_us"] = json!(now_us());
        if document["status"] != "accepted" {
            let _ = self.store.save_document("analysis-results", &id, &document);
            return;
        }
        if let Ok(records) = result {
            let source = format!("analysis:{}", analysis["id"].as_str().unwrap());
            let identity = format!(
                "analysis-v1:{}:{}",
                analysis["package_digest"].as_str().unwrap(),
                input.fingerprint
            );
            if let Err(error) = self
                .store
                .publish_result(crate::publication::PublicationRequest {
                    run_id: &run_id,
                    source_key: &source,
                    input_identity: &identity,
                    records: &records,
                    metadata: Some(crate::publication::PublicationMetadata::Analysis {
                        execution_id: &id,
                        document: &document,
                    }),
                    deadline: Some(deadline.into_std()),
                })
            {
                let requested = self
                    .store
                    .document("analysis-results", &id)
                    .is_ok_and(|row| row["cancellation_requested"] == true);
                document["status"] = json!(if requested {
                    "cancelled"
                } else if Instant::now() >= deadline {
                    "timed_out"
                } else {
                    "failed"
                });
                document["cancellation_requested"] = json!(requested);
                document["diagnostics"] =
                    json!([format!("analysis output rejected atomically: {error:#}")]);
                let _ = self.store.save_document("analysis-results", &id, &document);
            }
        } else {
            let _ = self.store.save_document("analysis-results", &id, &document);
        }
    }
}

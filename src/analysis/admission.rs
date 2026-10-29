//! Engine-family admission and the handoff from preparation to an owned attempt.
use super::{
    Engine, blocking,
    input::{self, Input},
    package, process,
};
use crate::model::now_us;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use tokio::{
    sync::{OwnedMutexGuard, OwnedSemaphorePermit, watch},
    time::Instant,
};

struct Request {
    run_id: String,
    analysis_id: String,
    timeout: f64,
    started_at_us: u64,
    deadline: Instant,
}

struct Prepared {
    request: Request,
    analysis: Arc<Value>,
    input: Arc<Input>,
    timings: Value,
}

struct Admitted {
    prepared: Prepared,
    id: String,
    directory: PathBuf,
    document: Value,
    sender: watch::Sender<bool>,
    cancellation: watch::Receiver<bool>,
}

/// Only a fresh UUID directory created by this preparation may be rolled back.
/// Once metadata is committed, the durable attempt owns retention and cleanup.
struct UnadmittedDirectory(Option<PathBuf>);
impl Drop for UnadmittedDirectory {
    fn drop(&mut self) {
        if let Some(directory) = &self.0 {
            let _ = fs::remove_dir_all(directory);
        }
    }
}

impl Engine {
    pub async fn run(&self, params: &Value) -> Result<Value> {
        self.check_preparation(None)?;
        let run_id = params["run_id"]
            .as_str()
            .context("analysis run_id required")?
            .to_owned();
        let analysis_id = params["analysis_id"]
            .as_str()
            .context("analysis_id required")?
            .to_owned();
        let timeout = params["timeout_seconds"].as_f64().unwrap_or(60.0);
        if !timeout.is_finite() || timeout <= 0.0 || timeout > 86400.0 {
            bail!("analysis timeout_seconds must be positive and at most 86400");
        }
        let request = Request {
            run_id,
            analysis_id,
            timeout,
            started_at_us: now_us(),
            deadline: Instant::now() + Duration::from_secs_f64(timeout),
        };
        let permit = self.slots.clone().try_acquire_owned().map_err(|_| anyhow::anyhow!(
            "analysis capacity reached: at most {} attempts per shared engine family, including preparation and terminal retention/publication",
            self.maximum_in_flight
        ))?;
        self.check_preparation(Some(request.deadline))?;
        let engine = self.clone();
        // Dropping a request or its acknowledgement wait must not abandon a
        // committed attempt. This owned task settles preparation and hands the
        // permit to completion before it returns the acknowledgement.
        tokio::spawn(async move { engine.prepare_and_admit(request, permit).await })
            .await
            .context("analysis admission task failed")?
    }

    fn check_preparation(&self, deadline: Option<Instant>) -> Result<()> {
        if self.closing.load(Ordering::Acquire) {
            bail!("analysis service is shutting down");
        }
        if let Some(deadline) = deadline {
            process::check_deadline(deadline)?;
        }
        Ok(())
    }

    fn prepare(&self, request: Request) -> Result<Prepared> {
        self.check_preparation(Some(request.deadline))?;
        self.recover()?;
        self.check_preparation(Some(request.deadline))?;
        let analysis = self.store.document("analyses", &request.analysis_id)?;
        let phase = Instant::now();
        package::verify(&analysis, || self.check_preparation(Some(request.deadline)))?;
        let mut timings =
            json!({"initial_package_verification":phase.elapsed().as_micros() as u64});
        self.check_preparation(Some(request.deadline))?;
        let phase = Instant::now();
        let input = input::snapshot(&self.store, &analysis, &request.run_id, || {
            self.check_preparation(Some(request.deadline))
        })?;
        timings["input_snapshot"] = json!(phase.elapsed().as_micros() as u64);
        self.check_preparation(Some(request.deadline))?;
        Ok(Prepared {
            request,
            analysis: Arc::new(analysis),
            input: Arc::new(input),
            timings,
        })
    }

    fn admit(
        &self,
        mut prepared: Prepared,
        mut active: OwnedMutexGuard<HashMap<String, watch::Sender<bool>>>,
    ) -> Result<Admitted> {
        let request = &prepared.request;
        self.check_preparation(Some(request.deadline))?;
        let id = uuid::Uuid::new_v4().to_string();
        let directory = self.store.root().join("analysis-executions").join(&id);
        let phase = Instant::now();
        fs::create_dir_all(directory.parent().unwrap())?;
        fs::create_dir(&directory)?;
        let mut unadmitted = UnadmittedDirectory(Some(directory.clone()));
        fs::write(directory.join("input.json"), &prepared.input.bytes)?;
        prepared.timings["input_staging"] = json!(phase.elapsed().as_micros() as u64);
        self.check_preparation(Some(request.deadline))?;
        let analysis = &prepared.analysis;
        let input = &prepared.input;
        let document = json!({
            "id":id,"result_set_id":id,"analysis_id":request.analysis_id,"run_id":request.run_id,
            "version":analysis["version"],"package_digest":analysis["package_digest"],"status":"running",
            "created_at_us":now_us(),"attempt_started_at_us":request.started_at_us,"timeout_seconds":request.timeout,
            "deadline_us":request.started_at_us.saturating_add((request.timeout*1_000_000.0)as u64),
            "budget_scope":"preparation_setup_execution_verification_retention_publication_checkpoints",
            "phase_timings_us":prepared.timings,"owner":self.owner,"input_result_sets":input.result_ids,
            "input_fingerprint":input.fingerprint,"diagnostics":[],"execution_dir":directory,
        });
        // The active lock makes this commit + local ownership handoff atomic
        // relative to shutdown's closing gate and cancellation enumeration.
        self.store
            .save_document("analysis-results", &id, &document)?;
        let (sender, cancellation) = watch::channel(false);
        active.insert(id.clone(), sender.clone());
        unadmitted.0.take();
        Ok(Admitted {
            prepared,
            id,
            directory,
            document,
            sender,
            cancellation,
        })
    }

    async fn prepare_and_admit(
        &self,
        request: Request,
        permit: OwnedSemaphorePermit,
    ) -> Result<Value> {
        let engine = self.clone();
        let prepared = blocking(move || engine.prepare(request)).await?;
        let active = self.active.clone().lock_owned().await;
        let engine = self.clone();
        let admitted = blocking(move || engine.admit(prepared, active)).await?;
        let Admitted {
            prepared,
            id,
            directory,
            document,
            sender,
            cancellation,
        } = admitted;
        let monitor = self.clone();
        let monitor_id = id.clone();
        tokio::spawn(async move {
            loop {
                let store = monitor.store.clone();
                let query_id = monitor_id.clone();
                match blocking(move || store.document("analysis-results", &query_id)).await {
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
                .complete(
                    prepared.analysis,
                    prepared.input,
                    document,
                    directory,
                    cancellation,
                    prepared.request.deadline,
                )
                .await;
            engine.active.lock().await.remove(&execution_id);
            drop(permit);
        });
        Ok(json!({"result_set_id":id,"status":"running","diagnostics":[]}))
    }
}

//! Recording-family admission, preparation and durable ownership handoff.
use super::{Engine, StartRequest, blocking, correctness, identity};
use crate::collection;
use crate::model::{
    now_us,
    runs::{Run, RunStatus},
};
use anyhow::{Context, Result, bail};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::atomic::Ordering};
use tokio::sync::{OwnedMutexGuard, OwnedSemaphorePermit, watch};

struct Prepared {
    run: Run,
    request: StartRequest,
}

impl Engine {
    pub async fn start(&self, request: StartRequest) -> Result<Run> {
        self.check_admission()?;
        if request.command.is_empty() || request.command[0].is_empty() {
            bail!("a nonempty command is required");
        }
        if request
            .timeout_seconds
            .is_some_and(|s| !s.is_finite() || s <= 0.0 || s > 86400.0)
        {
            bail!("timeout_seconds must be positive and at most 86400");
        }
        let permit = self.slots.clone().try_acquire_owned().map_err(|_| {
            crate::model::CapacityExceeded::new(
                "shared_recording_engine_family",
                self.maximum_in_flight,
            )
        })?;
        self.check_admission()?;
        let engine = self.clone();
        // A disconnected acknowledgement must not detach a committed recording
        // from its owner, nor release capacity while preparation still runs.
        tokio::spawn(async move { engine.prepare_and_admit(request, permit).await })
            .await
            .context("recording admission task failed")?
    }

    fn check_admission(&self) -> Result<()> {
        if self.closing.load(Ordering::Acquire) {
            bail!("recording engine is shutting down");
        }
        Ok(())
    }

    fn prepare(&self, request: StartRequest) -> Result<Prepared> {
        self.check_admission()?;
        let cwd = request
            .cwd
            .clone()
            .unwrap_or(std::env::current_dir()?)
            .canonicalize()
            .context("working directory does not exist")?;
        if !cwd.is_dir() {
            bail!("working directory must be a directory");
        }
        let recipe = correctness::recipe(&request)?;
        let environment = identity::environment(&request.env)?;
        let inputs = identity::workload_inputs(&cwd, &request.input_files)?;
        self.check_admission()?;
        let mut resolved_command = request.command.clone();
        if let Some(executable) =
            identity::resolve_with_environment(&request.command[0], &cwd, &request.env)
        {
            resolved_command[0] = executable.display().to_string();
        }
        let runtime = request
            .runtime
            .clone()
            .unwrap_or_else(|| detect_runtime(&request.command[0]));
        let runtime = if runtime.eq_ignore_ascii_case("none") {
            "none".to_owned()
        } else {
            collection::select_runtime(&runtime)
                .map(|selected| selected.id.to_owned())
                .unwrap_or(runtime)
        };
        let mut run = Run {
            id: uuid::Uuid::new_v4().to_string(),
            name: request
                .name
                .clone()
                .unwrap_or_else(|| request.command.join(" ")),
            status: RunStatus::Running,
            command: request.command.clone(),
            cwd: cwd.display().to_string(),
            runtime,
            started_at_us: now_us(),
            finished_at_us: None,
            exit_code: None,
            diagnostics: vec![],
            source: identity::source(&cwd, &resolved_command, Some(self.store.root())),
            correctness: correctness::pending(&recipe),
        };
        self.check_admission()?;
        run.source["environment_fingerprint"] = environment["fingerprint"].clone();
        run.source["environment"] = environment;
        run.source["workload_inputs"] = inputs;
        run.source["correctness_recipe"] = recipe.clone();
        run.source["correctness_recipe_fingerprint"] = json!(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&recipe)?)
        ));
        run.source["owner"] = serde_json::to_value(&self.owner)?;
        run.source["target_outcome"] = json!({"status":"not_started","exit_code":null});
        run.source["admission"] = json!({"scope":"shared_recording_engine_family","maximum_in_flight":self.maximum_in_flight,"permit_scope":"preparation_execution_terminal_retention_and_active_cleanup","limitations":"Separate Engine instances and processes are independent; this is not a memory or store-wide quota"});
        Ok(Prepared { run, request })
    }

    fn admit(
        &self,
        prepared: &Prepared,
        mut active: OwnedMutexGuard<HashMap<String, watch::Sender<bool>>>,
    ) -> Result<(watch::Sender<bool>, watch::Receiver<bool>)> {
        self.check_admission()?;
        let (sender, cancellation) = watch::channel(false);
        self.store.save_run(&prepared.run)?;
        active.insert(prepared.run.id.clone(), sender.clone());
        Ok((sender, cancellation))
    }

    async fn prepare_and_admit(
        &self,
        request: StartRequest,
        permit: OwnedSemaphorePermit,
    ) -> Result<Run> {
        let engine = self.clone();
        let prepared = blocking(move || engine.prepare(request)).await?;
        let active = self.active.clone().lock_owned().await;
        let engine = self.clone();
        let (prepared, sender, cancellation) = blocking(move || {
            let (sender, cancellation) = engine.admit(&prepared, active)?;
            Ok((prepared, sender, cancellation))
        })
        .await?;
        let engine = self.clone();
        let returned = prepared.run.clone();
        tokio::spawn(async move {
            let Prepared { run, request } = prepared;
            let monitor = engine.monitor_cancellation(run.id.clone(), sender);
            let executing = engine.clone();
            let executing_run = run.clone();
            let result = tokio::spawn(async move {
                executing
                    .execute(executing_run, request, cancellation)
                    .await
            })
            .await
            .context("recording execution task failed")
            .and_then(|result| result);
            if let Err(error) = monitor.finish().await {
                eprintln!("recording cancellation monitor failed: {error}");
            }
            if let Err(error) = result {
                let store = engine.store.clone();
                let failed_run = run.clone();
                if let Err(save) = blocking(move || {
                    let mut failed = store.run(&failed_run.id).unwrap_or(failed_run);
                    failed.status = RunStatus::Failed;
                    failed.finished_at_us = Some(now_us());
                    failed.diagnostics.push(format!("{error:#}"));
                    store.save_run(&failed)
                })
                .await
                {
                    eprintln!("recording finalization failed: {save}");
                }
            }
            engine.active.lock().await.remove(&run.id);
            drop(permit);
        });
        Ok(returned)
    }
}

fn detect_runtime(command: &str) -> String {
    let name = std::path::Path::new(command)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    if name.contains("graal") {
        "graalpy"
    } else if name.contains("pypy") {
        "pypy"
    } else if name.contains("python") {
        "cpython"
    } else if name == "node" {
        "node"
    } else if name == "bun" {
        "bun"
    } else if name == "deno" {
        "deno"
    } else {
        "native"
    }
    .into()
}

mod admission;
mod correctness;
mod finishing;
mod identity;
mod launch;
mod lifecycle;
pub(crate) mod ownership;
pub(crate) use crate::host::process;
mod process_identity;
mod resources;
mod retention;
mod sdk;

use crate::{
    collection::{self, CaptureRequest, native},
    model::{
        now_us,
        runs::{Run, RunStatus},
    },
    storage::Store,
};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    process::Stdio,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use tokio::{
    process::Command,
    sync::{Mutex, Semaphore, watch},
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRequest {
    pub name: Option<String>,
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub runtime: Option<String>,
    pub timeout_seconds: Option<f64>,
    pub expected_exit_code: Option<i32>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub input_files: Vec<PathBuf>,
    pub expected_stdout: Option<String>,
    pub expected_stdout_sha256: Option<String>,
}

/// Owned blocking units settle before their recording releases admission.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .context("recording blocking unit failed")?
}

#[derive(Clone)]
pub struct Engine {
    store: Store,
    active: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    closing: Arc<AtomicBool>,
    owner: ownership::Owner,
    slots: Arc<Semaphore>,
    maximum_in_flight: usize,
}

impl Engine {
    pub fn new(store: Store) -> Self {
        Self::with_admission_limit(store, 4)
            .expect("the default recording admission limit is valid")
    }
    /// Bound preparation, execution and terminal retention for this Engine and
    /// its clones. Separate Engines are independent; this is not a memory or
    /// cross-process/store quota.
    pub fn with_admission_limit(store: Store, maximum_in_flight: usize) -> Result<Self> {
        anyhow::ensure!(
            maximum_in_flight > 0 && maximum_in_flight <= Semaphore::MAX_PERMITS,
            "recording admission limit must be positive and supported by the semaphore"
        );
        if let Err(error) = ownership::recover_abandoned(&store) {
            eprintln!("recording owner recovery failed: {error:#}");
        }
        Ok(Self {
            store,
            active: Arc::new(Mutex::new(HashMap::new())),
            closing: Arc::new(AtomicBool::new(false)),
            owner: ownership::Owner::current(),
            slots: Arc::new(Semaphore::new(maximum_in_flight)),
            maximum_in_flight,
        })
    }
    pub fn store(&self) -> &Store {
        &self.store
    }

    async fn execute(
        &self,
        run: Run,
        request: StartRequest,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<()> {
        let engine = self.clone();
        let planning_request = request.clone();
        let planning_cancel = cancel.clone();
        let Some(launch::Prepared {
            mut run,
            capture,
            plan,
            stdout,
            stderr,
        }) =
            blocking(move || engine.prepare_launch(run, planning_request, planning_cancel)).await?
        else {
            return Ok(());
        };
        let directory = capture.artifact_dir.clone();
        let mut command = Command::new(
            plan.command
                .first()
                .context("collector produced an empty command")?,
        );
        command
            .args(&plan.command[1..])
            .envs(&request.env)
            .envs(&plan.env)
            .env("HUNIN_RUN_ID", &run.id)
            .env("HUNIN_RECORD_DIR", directory.join("sdk"))
            .current_dir(&run.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true);
        let execution_started_at_us = now_us();
        let (mut child, mut tree) = process::spawn(&mut command).await?;
        let pid = child.id().context("target process has no PID")?;
        // A collector may retain evidence about the actual supervised process.
        // Failure to retain that evidence must not stop or replace the target.
        #[cfg(windows)]
        let native_wrapper_birth = process_identity::birth(pid);
        let native_collection = native::handles_runtime(&run.runtime);
        let collector_wrapper = plan.command[0] != capture.command[0]
            || matches!(run.runtime.as_str(), "go-test" | "go-trace");
        let store = self.store.clone();
        let started_capture = capture.clone();
        let started = blocking(move || {
            let failure = plan
                .on_started(&started_capture, pid)
                .err()
                .map(|error| format!("collector start evidence failed: {error:#}"));
            if let Some(diagnostic) = &failure {
                run.source["collection_start"] = json!({"status":"failed","diagnostic":diagnostic});
                run.diagnostics.push(diagnostic.clone());
            }
            run.source["supervised_execution_started_at_us"] = json!(execution_started_at_us);
            run.source["supervised_execution_scope"] = json!(if collector_wrapper {
                "collector_and_target"
            } else {
                "target_process"
            });
            run.source["supervised_pid"] = json!(pid);
            run.source["target_outcome"] = json!({"status":"running","supervised_pid":pid});
            store.save_run(&run)?;
            Ok((run, plan, failure))
        })
        .await;
        let (mut run, plan, collector_start_failure) = match started {
            Ok(value) => value,
            Err(error) => {
                #[cfg(windows)]
                native::request_stop(
                    &directory,
                    &capture.runtime,
                    pid,
                    native_wrapper_birth.as_deref(),
                )
                .await;
                tree.force();
                return Err(error);
            }
        };
        let mut metrics = resources::Retention::default();
        let (observer, mut observations) =
            resources::Observer::start(pid, run.id.clone(), collector_wrapper);
        let mut observations_open = true;
        let timeout = tokio::time::sleep(Duration::from_secs_f64(
            request.timeout_seconds.unwrap_or(86400.0),
        ));
        tokio::pin!(timeout);
        let mut canceled = false;
        let mut timed_out = false;
        let status: Result<std::process::ExitStatus> = loop {
            tokio::select! {
                status=child.wait()=>{
                    // The elevated ETW owner can share this Job. Even an
                    // unexpected wrapper exit must give it cleanup time before
                    // the common force/drop path closes the remaining tree.
                    #[cfg(windows)]
                    native::request_stop(&directory,&run.runtime,pid,native_wrapper_birth.as_deref()).await;
                    break status.context("wait for supervised recording process");
                }
                _=cancel.changed()=>{
                    canceled=true;
                    #[cfg(windows)]
                    native::request_stop(&directory,&run.runtime,pid,native_wrapper_birth.as_deref()).await;
                    match tree.stop(&mut child,run.runtime!="none").await {
                        Ok((status,forced))=>{
                            if forced {run.diagnostics.push("collector/target did not stop within the graceful deadline; owned process tree forced closed".into());}
                            break Ok(status);
                        }
                        Err(error)=>break Err(error),
                    }
                }
                _=&mut timeout=>{
                    timed_out=true;
                    #[cfg(windows)]
                    native::request_stop(&directory,&run.runtime,pid,native_wrapper_birth.as_deref()).await;
                    match tree.stop(&mut child,run.runtime!="none").await {
                        Ok((status,forced))=>{
                            if forced {run.diagnostics.push("collector/target did not stop within the graceful deadline; owned process tree forced closed".into());}
                            break Ok(status);
                        }
                        Err(error)=>break Err(error),
                    }
                }
                rows=observations.recv(),if observations_open=>{
                    if let Some(rows)=rows {metrics.extend(rows);} else {observations_open=false;}
                }
            }
        };
        tree.force();
        let execution_finished_at_us = now_us();
        run.source["supervised_execution_finished_at_us"] = json!(execution_finished_at_us);
        run.source["supervised_elapsed_us"] =
            json!(execution_finished_at_us.saturating_sub(execution_started_at_us));
        let observation = observer.finish().await;
        if let Some(error) = &observation.worker_error {
            run.diagnostics
                .push(format!("resource observer stopped unexpectedly: {error}"));
        }
        if observation.delivery.discarded_rows() > 0 {
            run.diagnostics.push(format!("{} generated resource observation rows were not delivered; see resource_observation.queue_delivery. This does not quantify unobserved CPU weights or OS gaps",observation.delivery.discarded_rows()));
        }
        while let Ok(rows) = observations.try_recv() {
            metrics.extend(rows);
        }
        // Stop and join the observer even when waiting/stopping the target
        // failed; it must not outlive the recording's admission ownership.
        let status = status?;
        #[cfg(windows)]
        if native_collection {
            let cleanup = native::wait_for_cleanup(
                &directory,
                pid,
                native_wrapper_birth.as_deref(),
                Duration::from_secs(8),
            )
            .await;
            if cleanup["acknowledged"] != true {
                run.diagnostics.push(format!("Native ETW helper cleanup was not confirmed: {}; inspect retained xperf-session evidence before retrying", cleanup["reason"]));
            }
            run.source["native_session_cleanup"] = cleanup;
        }
        run.source["resource_observation"] = json!({"scope":"observed processes in supervised tree","requested_interval_us":100000,"implementation":"CPU/memory-only process discovery on a blocking worker, bracketed by native process birth identities","limitations":"observer work can perturb the target; refresh duration and actual point intervals are recorded; missing or changing birth identities are omitted; newly discovered children need a subsequent stable observation; short-lived processes or final cumulative values may be missed"});
        run.source["resource_observation"]["queue_delivery"] =
            serde_json::to_value(&observation.delivery)?;
        run.source["resource_observation"]["observer_state"] =
            json!(if observation.worker_error.is_some() {
                "failed"
            } else {
                "stopped"
            });
        run.source["process_elapsed_limitation"] = json!(if collector_wrapper {
            "includes collector startup/shutdown, observer overhead and supervised command; not isolated target elapsed"
        } else if run.runtime == "none" {
            "includes runtime startup/shutdown and observer overhead; process wall interval, not isolated function time"
        } else {
            "includes runtime profiling and observer overhead; not an unprofiled speed measurement"
        });
        let target_outcome = if native_collection {
            let capture_directory = directory.clone();
            let collector = PathBuf::from(&plan.command[0]);
            let outcome = match tokio::task::spawn_blocking(move || {
                native::read_target_outcome(&capture_directory, &collector)
            })
            .await
            {
                Ok(Ok(outcome)) => outcome,
                result => {
                    let reason = match result {
                        Ok(Err(error)) => format!("{error:#}"),
                        Err(error) => error.to_string(),
                        _ => unreachable!(),
                    };
                    run.diagnostics.push(format!("Native target wait status unavailable: {reason}; collector exit is not substituted for target exit"));
                    native::unknown_target_outcome(&reason)
                }
            };
            run.source["collector_outcome"] = json!({"state":if status.code().is_some(){"exited"}else{"terminated"},"exit_code":status.code(),"scope":"supervised native profiler process","exit_status_consistent_with_target":outcome["exit_code"].as_i64().map(|code|Some(code)==status.code().map(i64::from))});
            Some(outcome)
        } else {
            None
        };
        let engine = self.clone();
        let target = finishing::Target {
            exit_code: status.code(),
            outcome: target_outcome,
            canceled,
            timed_out,
        };
        let capture = finishing::Capture {
            request: capture,
            plan,
            start_failure: collector_start_failure,
        };
        blocking(move || engine.finish(run, target, capture, metrics)).await
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

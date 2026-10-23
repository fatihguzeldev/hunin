mod correctness;
mod identity;
pub(crate) mod ownership;
pub(crate) mod process;
mod resources;

use crate::{
    collection::{self, CaptureRequest},
    model::{now_us, runs::Run},
    storage::Store,
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    process::Command,
    sync::{Mutex, watch},
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

#[derive(Clone)]
pub struct Engine {
    store: Store,
    active: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    closing: Arc<AtomicBool>,
    owner: ownership::Owner,
}

impl Engine {
    pub fn new(store: Store) -> Self {
        if let Err(error) = ownership::recover_abandoned(&store) {
            eprintln!("recording owner recovery failed: {error:#}");
        }
        Self {
            store,
            active: Arc::new(Mutex::new(HashMap::new())),
            closing: Arc::new(AtomicBool::new(false)),
            owner: ownership::Owner::current(),
        }
    }
    pub fn store(&self) -> &Store {
        &self.store
    }

    pub async fn start(&self, request: StartRequest) -> Result<Run> {
        if self.closing.load(Ordering::Acquire) {
            bail!("recording engine is shutting down");
        }
        if request.command.is_empty() || request.command[0].is_empty() {
            bail!("a nonempty command is required");
        }
        if request
            .timeout_seconds
            .is_some_and(|s| !s.is_finite() || s <= 0.0)
        {
            bail!("timeout_seconds must be positive and finite");
        }
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
        let mut run = Run {
            id: uuid::Uuid::new_v4().to_string(),
            name: request
                .name
                .clone()
                .unwrap_or_else(|| request.command.join(" ")),
            status: "running".into(),
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
        let (tx, rx) = watch::channel(false);
        let mut active = self.active.lock().await;
        if self.closing.load(Ordering::Acquire) {
            bail!("recording engine is shutting down");
        }
        self.store.save_run(&run)?;
        active.insert(run.id.clone(), tx);
        drop(active);
        let engine = self.clone();
        let task_run = run.clone();
        tokio::spawn(async move {
            if let Err(error) = engine.execute(task_run.clone(), request, rx).await {
                let mut failed = engine.store.run(&task_run.id).unwrap_or(task_run.clone());
                failed.status = "failed".into();
                failed.finished_at_us = Some(now_us());
                failed.diagnostics.push(format!("{error:#}"));
                if let Err(save) = engine.store.save_run(&failed) {
                    eprintln!("recording finalization failed: {save}");
                }
            }
            engine.active.lock().await.remove(&task_run.id);
        });
        Ok(run)
    }

    pub async fn cancel(&self, id: &str) -> Result<Run> {
        let run = self.store.run(id)?;
        if run.status == "running" {
            self.store.save_document(
                "cancellations",
                id,
                &json!({"run_id":id,"requested_at_us":now_us()}),
            )?;
            let active = self.active.lock().await;
            if let Some(sender) = active.get(id) {
                let _ = sender.send(true);
            }
        }
        Ok(run)
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.closing.store(true, Ordering::Release);
        let ids = self.active.lock().await.keys().cloned().collect::<Vec<_>>();
        for id in &ids {
            self.cancel(id).await?;
        }
        for id in ids {
            self.wait(&id).await?;
        }
        Ok(())
    }

    pub async fn wait(&self, id: &str) -> Result<Run> {
        loop {
            let run = self.store.run(id)?;
            if run.status != "running" {
                return Ok(run);
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    async fn execute(
        &self,
        mut run: Run,
        request: StartRequest,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<()> {
        let directory = self.store.root().join("captures").join(&run.id);
        fs::create_dir_all(directory.join("sdk"))?;
        crate::source::retain(&self.store, &mut run)?;
        let mut capture_command = run.command.clone();
        if let Some(path) = run.source["executable"].as_str() {
            capture_command[0] = path.to_owned();
        }
        let capture = CaptureRequest {
            command: capture_command.clone(),
            cwd: PathBuf::from(&run.cwd),
            artifact_dir: directory.clone(),
            run_id: run.id.clone(),
            runtime: run.runtime.clone(),
            env: request.env.clone(),
        };
        let plan = if run.runtime == "none" {
            collection::CapturePlan {
                command: capture_command,
                env: Default::default(),
                diagnostics: vec![
                    "Automatic profiling explicitly disabled for this recording".into(),
                ],
                runtime_version: None,
            }
        } else {
            collection::prepare(&capture)?
        };
        let normalize = |value: &str| {
            value
                .replace(&directory.to_string_lossy().to_string(), "<artifacts>")
                .replace(&run.id, "<run>")
        };
        let settings = json!({"runtime":run.runtime,"runtime_version":plan.runtime_version,"command":plan.command.iter().map(|v|normalize(v)).collect::<Vec<_>>(),"environment":plan.env.iter().map(|(k,v)|(k.clone(),normalize(v))).collect::<std::collections::BTreeMap<_,_>>()});
        run.source["collector_settings_fingerprint"] = json!(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&settings)?)
        ));
        run.source["collector_settings"] = settings;
        run.diagnostics.extend(plan.diagnostics);
        run.source["runtime_version"] = json!(plan.runtime_version);
        self.store.save_run(&run)?;
        if *cancel.borrow() || self.store.document("cancellations", &run.id).is_ok() {
            run.status = "canceled".into();
            run.finished_at_us = Some(now_us());
            run.source["target_outcome"] =
                json!({"status":"not_started","reason":"canceled before launch"});
            run.source["collection_status"] = json!("canceled");
            self.store.save_run(&run)?;
            return Ok(());
        }
        let stdout = directory.join("stdout.txt");
        let stderr = directory.join("stderr.txt");
        let mut command = Command::new(
            plan.command
                .first()
                .context("collector produced an empty command")?,
        );
        command
            .args(&plan.command[1..])
            .envs(&request.env)
            .envs(plan.env)
            .env("HUNIN_RUN_ID", &run.id)
            .env("HUNIN_RECORD_DIR", directory.join("sdk"))
            .current_dir(&run.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(fs::File::create(&stdout)?))
            .stderr(Stdio::from(fs::File::create(&stderr)?))
            .kill_on_drop(true);
        let execution_started_at_us = now_us();
        let (mut child, mut tree) = process::spawn(&mut command).await?;
        let pid = child.id().context("target process has no PID")?;
        let collector_wrapper = plan.command[0] != capture.command[0]
            || matches!(run.runtime.as_str(), "go-test" | "go-trace");
        run.source["supervised_execution_started_at_us"] = json!(execution_started_at_us);
        run.source["supervised_execution_scope"] = json!(if collector_wrapper {
            "collector_and_target"
        } else {
            "target_process"
        });
        run.source["supervised_pid"] = json!(pid);
        run.source["target_outcome"] = json!({"status":"running","supervised_pid":pid});
        self.store.save_run(&run)?;
        let mut metrics = Vec::new();
        let (observer, mut observations) =
            resources::Observer::start(pid, run.id.clone(), collector_wrapper);
        let mut observations_open = true;
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        let timeout = tokio::time::sleep(Duration::from_secs_f64(
            request.timeout_seconds.unwrap_or(86400.0),
        ));
        tokio::pin!(timeout);
        let mut canceled = false;
        let mut timed_out = false;
        let status = loop {
            tokio::select! {
                status=child.wait()=>{break status?;}
                _=cancel.changed()=>{canceled=true;let(status,forced)=tree.stop(&mut child,run.runtime!="none").await?;if forced{run.diagnostics.push("collector/target did not stop within the graceful deadline; owned process tree forced closed".into());}break status;}
                _=&mut timeout=>{timed_out=true;let(status,forced)=tree.stop(&mut child,run.runtime!="none").await?;if forced{run.diagnostics.push("collector/target did not stop within the graceful deadline; owned process tree forced closed".into());}break status;}
                rows=observations.recv(),if observations_open=>{if let Some(rows)=rows{metrics.extend(rows);}else{observations_open=false;}}
                _=tick.tick()=>{
                    if self.store.document("cancellations",&run.id).is_ok(){canceled=true;let(status,forced)=tree.stop(&mut child,run.runtime!="none").await?;if forced{run.diagnostics.push("persistent cancellation required forced process-tree cleanup after the graceful deadline".into());}break status;}
                }
            }
        };
        tree.force();
        let execution_finished_at_us = now_us();
        run.source["supervised_execution_finished_at_us"] = json!(execution_finished_at_us);
        run.source["supervised_elapsed_us"] =
            json!(execution_finished_at_us.saturating_sub(execution_started_at_us));
        if let Err(error) = observer.finish().await {
            run.diagnostics
                .push(format!("resource observer stopped unexpectedly: {error}"));
        }
        while let Ok(rows) = observations.try_recv() {
            metrics.extend(rows);
        }
        run.source["resource_observation"] = json!({"scope":"observed processes in supervised tree","requested_interval_us":100000,"implementation":"CPU/memory-only process discovery on a blocking worker","limitations":"observer work can perturb the target; refresh duration and actual point intervals are recorded; short-lived processes or final cumulative values may be missed"});
        run.source["process_elapsed_limitation"] = json!(if collector_wrapper {
            "includes collector startup/shutdown, observer overhead and supervised command; not isolated target elapsed"
        } else if run.runtime == "none" {
            "includes runtime startup/shutdown and observer overhead; process wall interval, not isolated function time"
        } else {
            "includes runtime profiling and observer overhead; not an unprofiled speed measurement"
        });
        run.exit_code = status.code();
        run.finished_at_us = Some(now_us());
        run.correctness = correctness::evaluate(
            &run.source["correctness_recipe"],
            &stdout,
            status.code(),
            canceled,
            timed_out,
        )?;
        if !canceled && !timed_out {
            for check in run.correctness["checks"].as_array().unwrap() {
                if check["passed"] == false {
                    run.diagnostics.push(format!(
                        "target correctness check failed: {}",
                        check["kind"].as_str().unwrap_or("unknown")
                    ));
                }
            }
        }
        run.status = if canceled {
            "canceled"
        } else if run.correctness["passed"] != true {
            "failed"
        } else {
            "completed"
        }
        .into();
        run.source["target_outcome"] =
            json!({"status":run.status,"exit_code":run.exit_code,"correctness":run.correctness});
        run.source["collection_status"] = json!(if run.runtime == "none" {
            "disabled"
        } else {
            "finalizing"
        });
        // Keep waiters on the recording until preservation and ingestion finish,
        // while retaining the actual target outcome if finalization later fails.
        let mut finalizing = run.clone();
        finalizing.status = "running".into();
        finalizing.finished_at_us = None;
        self.store.save_run(&finalizing)?;
        if timed_out {
            run.diagnostics
                .push("recording timeout; target process group terminated".into());
        }
        self.store.add_artifact(&run.id, &stdout, "stdout")?;
        self.store.add_artifact(&run.id, &stderr, "stderr")?;
        preserve_capture(&self.store, &run.id, &directory)?;
        if !metrics.is_empty() {
            self.store
                .accept(&run.id, "process-resources-v1", &metrics)?;
        }
        if run.runtime != "none" {
            match collection::collect(&capture) {
                Ok(profiles) => {
                    for profile in profiles {
                        let raw =
                            self.store
                                .add_artifact(&run.id, &profile.source, &profile.format)?;
                        for sidecar in &profile.sidecars {
                            self.store
                                .add_artifact(&run.id, sidecar, "profile-sidecar")?;
                        }
                        self.store.accept_import(
                            &run.id,
                            &format!("import-v1:{}", raw.sha256),
                            &profile.input_fingerprint,
                            &profile.records,
                        )?;
                        run.diagnostics.extend(profile.diagnostics);
                    }
                    run.source["collection_status"] = json!("completed");
                }
                Err(error) => {
                    run.diagnostics
                        .push(format!("automatic collection incomplete: {error:#}"));
                    run.source["collection_status"] = json!("failed");
                    if run.status == "completed" {
                        run.status = "failed".into();
                    }
                }
            }
        }
        let mut sdk = Vec::new();
        for entry in fs::read_dir(directory.join("sdk"))? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "jsonl") {
                self.store.add_artifact(&run.id, &path, "hunin-jsonl")?;
                for line in std::io::BufRead::lines(std::io::BufReader::new(fs::File::open(path)?))
                {
                    let line = line?;
                    if !line.trim().is_empty() {
                        sdk.push(serde_json::from_str::<Value>(&line)?);
                    }
                }
            }
        }
        if !sdk.is_empty() {
            if let Err(error) = self.store.accept(&run.id, "sdk-spool-v1", &sdk) {
                run.diagnostics
                    .push(format!("SDK result set rejected atomically: {error:#}"));
            }
        }
        run.finished_at_us = Some(now_us());
        self.store.save_run(&run)?;
        Ok(())
    }
}

fn preserve_capture(store: &Store, run_id: &str, path: &std::path::Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            preserve_capture(store, run_id, &entry.path())?;
        } else if kind.is_file() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let format = collection::profile_format(&path).unwrap_or_else(|| {
                if name.ends_with(".jsonl") {
                    "hunin-jsonl"
                } else if name.ends_with(".meta.json")
                    || name.ends_with(".syms.json")
                    || name == "go-tool.json"
                {
                    "profile-sidecar"
                } else if name == "stdout.txt" {
                    "stdout"
                } else if name == "stderr.txt" {
                    "stderr"
                } else {
                    "collector-artifact"
                }
            });
            store.add_artifact(run_id, &path, format)?;
        }
    }
    Ok(())
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

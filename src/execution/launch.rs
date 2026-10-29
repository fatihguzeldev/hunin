//! Blocking source retention, collector planning and target output setup.
use super::{CaptureRequest, Engine, Run, RunStatus, StartRequest, collection};
use anyhow::Result;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};
use tokio::sync::watch;

pub(super) struct Prepared {
    pub run: Run,
    pub capture: CaptureRequest,
    pub plan: collection::CapturePlan,
    pub stdout: fs::File,
    pub stderr: fs::File,
}

impl Engine {
    pub(super) fn prepare_launch(
        &self,
        mut run: Run,
        request: StartRequest,
        cancel: watch::Receiver<bool>,
    ) -> Result<Option<Prepared>> {
        let directory = self.store.root().join("captures").join(&run.id);
        fs::create_dir_all(directory.join("sdk"))?;
        crate::source::retain(&self.store, &mut run)?;
        let mut command = run.command.clone();
        if let Some(path) = run.source["launch_executable"]
            .as_str()
            .or_else(|| run.source["executable"].as_str())
        {
            command[0] = path.to_owned();
        }
        let capture = CaptureRequest {
            command: command.clone(),
            cwd: PathBuf::from(&run.cwd),
            artifact_dir: directory.clone(),
            run_id: run.id.clone(),
            runtime: run.runtime.clone(),
            env: request.env,
        };
        let plan = if run.runtime == "none" {
            collection::CapturePlan {
                command,
                env: Default::default(),
                diagnostics: vec![
                    "Automatic profiling explicitly disabled for this recording".into(),
                ],
                runtime_version: None,
                collector_identity: None,
                session_identity: Default::default(),
                output: Default::default(),
            }
        } else {
            collection::prepare(&capture)?
        };
        let settings = plan.settings(&capture);
        if let Some(identity) = &plan.collector_identity {
            run.source["collector_identity"] = identity.clone();
        }
        run.source["collector_settings_fingerprint"] = json!(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&settings)?)
        ));
        run.source["collector_settings"] = settings;
        run.diagnostics.extend(plan.diagnostics.clone());
        run.source["runtime_version"] = json!(plan.runtime_version);
        self.store.save_run(&run)?;
        if *cancel.borrow() || self.store.document("cancellations", &run.id).is_ok() {
            run.status = RunStatus::Canceled;
            run.finished_at_us = Some(crate::model::now_us());
            run.source["target_outcome"] =
                json!({"status":"not_started","reason":"canceled before launch"});
            run.source["collection_status"] = json!("canceled");
            self.store.save_run(&run)?;
            return Ok(None);
        }
        let stdout = fs::File::create(directory.join("stdout.txt"))?;
        let stderr = fs::File::create(directory.join("stderr.txt"))?;
        Ok(Some(Prepared {
            run,
            capture,
            plan,
            stdout,
            stderr,
        }))
    }
}

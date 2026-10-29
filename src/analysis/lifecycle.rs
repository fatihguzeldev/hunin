use super::Engine;
use crate::{execution::ownership::is_proven_absent, model::now_us};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

impl Engine {
    pub(super) fn retain(&self, document: &mut Value, directory: &Path) -> Result<()> {
        fs::write(
            directory.join("attempt.json"),
            serde_json::to_vec_pretty(document)?,
        )?;
        let mut artifacts = Vec::new();
        for file in fs::read_dir(directory)? {
            let path = file?.path();
            if !fs::symlink_metadata(&path)?.is_file() {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if ![
                "input.json",
                "stdout.jsonl",
                "stderr.txt",
                "dependencies.json",
                "attempt.json",
            ]
            .contains(&name)
                && !(name.starts_with("setup-")
                    && (name.ends_with(".stdout") || name.ends_with(".stderr")))
            {
                continue;
            }
            let format = if name == "input.json" {
                "analysis-input"
            } else {
                "analysis-output"
            };
            artifacts.push(serde_json::to_value(self.store.add_artifact(
                document["run_id"].as_str().unwrap(),
                &path,
                format,
            )?)?);
        }
        document["raw_artifacts"] = json!(artifacts);
        Ok(())
    }
    pub fn recover(&self) -> Result<usize> {
        let mut count = 0;
        for mut row in self
            .store
            .documents_with_status("analysis-results", "running")?
        {
            if row["status"] != "running"
                || row["owner"]["instance_id"] == self.owner.instance_id
                || !is_proven_absent(&row["owner"])
            {
                continue;
            }
            row["status"] = json!("interrupted");
            row["finished_at_us"] = json!(now_us());
            row["diagnostics"] = json!([
                "Analyzer interrupted: its owning process is proven absent or its OS process lifetime changed. Partial evidence was retained; no stale PID was signaled and no output was accepted."
            ]);
            if let Some(directory) = row["execution_dir"].as_str().map(PathBuf::from)
                && let Err(error) = self.retain(&mut row, &directory)
            {
                row["diagnostics"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(format!(
                        "Partial artifact retention failed: {error:#}"
                    )));
            }
            self.store
                .save_document("analysis-results", row["id"].as_str().unwrap(), &row)?;
            count += 1;
        }
        Ok(count)
    }
    pub async fn cancel(&self, id: &str) -> Result<Value> {
        self.recover()?;
        let document = self.store.request_analysis_cancel(id)?;
        if let Some(sender) = self.active.lock().await.get(id) {
            let _ = sender.send(true);
        }
        Ok(
            json!({"execution_id":id,"status":document["status"],"diagnostics":if document["status"]=="running" {vec!["Cancellation requested; the owning service will stop its process tree before publication."]} else {vec![]}}),
        )
    }
    pub async fn wait(&self, id: &str) -> Result<Value> {
        loop {
            self.recover()?;
            let document = self.store.document("analysis-results", id)?;
            if document["status"] != "running" {
                return Ok(document);
            }
            if !self.active.lock().await.contains_key(id) {
                if document["owner"]["pid"].as_u64() == Some(std::process::id() as u64)
                    && document["owner"]["instance_id"] == self.owner.instance_id
                {
                    bail!(
                        "analysis task ended without a terminal result; inspect retained evidence"
                    );
                }
                if document["deadline_us"]
                    .as_u64()
                    .is_none_or(|deadline| now_us() > deadline.saturating_add(10_000_000))
                {
                    bail!(
                        "foreign analysis owner is live or unresolved but its bounded wait expired; inspect or request cancellation through that service"
                    );
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.closing
            .store(true, std::sync::atomic::Ordering::Release);
        let owned: Vec<_> = self
            .active
            .lock()
            .await
            .iter()
            .map(|(id, sender)| (id.clone(), sender.clone()))
            .collect();
        for (id, sender) in &owned {
            let _ = self.store.request_analysis_cancel(id);
            let _ = sender.send(true);
        }
        loop {
            let active = self.active.lock().await;
            if owned.iter().all(|(id, _)| !active.contains_key(id)) {
                return Ok(());
            }
            drop(active);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

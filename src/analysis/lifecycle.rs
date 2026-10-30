use super::{Engine, blocking, retention};
use crate::{host::process::owner::is_proven_absent, model::now_us};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

impl Engine {
    fn retain_recovered(&self, document: &mut Value, directory: &Path) -> Result<()> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        retention::retain(
            &self.store,
            document,
            directory,
            retention::Limits::default(),
            || {
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "analysis recovery evidence retention timed out"
                );
                Ok(())
            },
        )
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
                "Analyzer interrupted: its owning process is proven absent or its OS process lifetime changed. No stale PID was signaled, no output was accepted, and recovery did not remove the original execution directory. Any raw retention failure is reported separately."
            ]);
            if let Some(directory) = row["execution_dir"].as_str().map(PathBuf::from)
                && let Err(error) = self.retain_recovered(&mut row, &directory)
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
        let engine = self.clone();
        let query_id = id.to_owned();
        let document = blocking(move || {
            engine.recover()?;
            engine.store.request_analysis_cancel(&query_id)
        })
        .await?;
        if let Some(sender) = self.active.lock().await.get(id) {
            let _ = sender.send(true);
        }
        Ok(
            json!({"execution_id":id,"status":document["status"],"diagnostics":if document["status"]=="running" {vec!["Cancellation requested; the owning service will stop its process tree before publication."]} else {vec![]}}),
        )
    }
    pub async fn wait(&self, id: &str) -> Result<Value> {
        loop {
            let engine = self.clone();
            let query_id = id.to_owned();
            let document = blocking(move || {
                engine.recover()?;
                engine.store.document("analysis-results", &query_id)
            })
            .await?;
            if let Some(document) = self.wait_iteration(id, document).await? {
                return Ok(document);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    async fn wait_iteration(&self, id: &str, document: Value) -> Result<Option<Value>> {
        if document["status"] != "running" {
            return Ok(Some(document));
        }
        if !self.active.lock().await.contains_key(id) {
            // Completion commits its terminal document before removing local
            // ownership. The first read may precede that commit, so absence
            // must be followed by a fresh authoritative read before deciding
            // that an owned task disappeared without a terminal result.
            let store = self.store.clone();
            let query_id = id.to_owned();
            let document = blocking(move || store.document("analysis-results", &query_id)).await?;
            if document["status"] != "running" {
                return Ok(Some(document));
            }
            if document["owner"]["pid"].as_u64() == Some(std::process::id() as u64)
                && document["owner"]["instance_id"] == self.owner.instance_id
            {
                bail!("analysis task ended without a terminal result; inspect retained evidence");
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
        Ok(None)
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
            let _ = sender.send(true);
            let store = self.store.clone();
            let id = id.clone();
            let _ = blocking(move || store.request_analysis_cancel(&id)).await;
        }
        loop {
            let active = self.active.lock().await;
            if owned.iter().all(|(id, _)| !active.contains_key(id))
                && self.slots.available_permits() == self.maximum_in_flight
            {
                return Ok(());
            }
            drop(active);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;

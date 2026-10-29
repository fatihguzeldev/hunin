use super::{Engine, blocking};
use crate::model::{
    now_us,
    runs::{Run, RunStatus},
};
use anyhow::Result;
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{sync::watch, task::JoinHandle};

pub(super) struct CancellationMonitor {
    stopped: Arc<AtomicBool>,
    task: Option<JoinHandle<()>>,
}

impl CancellationMonitor {
    pub async fn finish(mut self) -> Result<(), tokio::task::JoinError> {
        self.stopped.store(true, Ordering::Release);
        self.task.take().unwrap().await
    }
}
impl Drop for CancellationMonitor {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

impl Engine {
    pub(super) fn monitor_cancellation(
        &self,
        id: String,
        sender: watch::Sender<bool>,
    ) -> CancellationMonitor {
        let stopped = Arc::new(AtomicBool::new(false));
        let check = stopped.clone();
        let store = self.store.clone();
        let task = tokio::spawn(async move {
            while !check.load(Ordering::Acquire) {
                let store = store.clone();
                let id = id.clone();
                let requested =
                    blocking(move || Ok(store.document("cancellations", &id).is_ok())).await;
                if check.load(Ordering::Acquire) {
                    break;
                }
                if matches!(requested, Ok(true)) {
                    let _ = sender.send(true);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        CancellationMonitor {
            stopped,
            task: Some(task),
        }
    }
    pub async fn cancel(&self, id: &str) -> Result<Run> {
        let store = self.store.clone();
        let id = id.to_owned();
        let run = blocking(move || {
            let run = store.run(&id)?;
            if run.status == RunStatus::Running {
                store.save_document(
                    "cancellations",
                    &id,
                    &json!({"run_id":id,"requested_at_us":now_us()}),
                )?;
            }
            Ok(run)
        })
        .await?;
        if run.status == RunStatus::Running
            && let Some(sender) = self.active.lock().await.get(&run.id)
        {
            let _ = sender.send(true);
        }
        Ok(run)
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.closing.store(true, Ordering::Release);
        let owned = self
            .active
            .lock()
            .await
            .iter()
            .map(|(id, sender)| (id.clone(), sender.clone()))
            .collect::<Vec<_>>();
        // Local owners receive cancellation even if persistent cancellation is
        // blocked or unavailable. Foreign Engines' recordings are untouched.
        for (id, sender) in owned {
            let _ = sender.send(true);
            let store = self.store.clone();
            let _ = blocking(move || {
                store.save_document(
                    "cancellations",
                    &id,
                    &json!({"run_id":id,"requested_at_us":now_us()}),
                )
            })
            .await;
        }
        loop {
            if self.active.lock().await.is_empty()
                && self.slots.available_permits() == self.maximum_in_flight
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub async fn wait(&self, id: &str) -> Result<Run> {
        loop {
            let store = self.store.clone();
            let id = id.to_owned();
            let run = blocking(move || store.run(&id)).await?;
            if run.status != RunStatus::Running {
                return Ok(run);
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_keeps_ownership_until_a_known_preparation_permit_is_released() {
        let root = tempfile::tempdir().unwrap();
        let engine =
            Engine::with_admission_limit(crate::storage::Store::open(root.path()).unwrap(), 1)
                .unwrap();
        let permit = engine.slots.clone().try_acquire_owned().unwrap();
        let closing = engine.clone();
        let shutdown = tokio::spawn(async move { closing.shutdown().await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !engine.closing.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            !shutdown.is_finished(),
            "shutdown cannot abandon a known owned preparation permit"
        );
        drop(permit);
        tokio::time::timeout(Duration::from_secs(2), shutdown)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(engine.slots.available_permits(), 1);
        assert!(engine.closing.load(Ordering::Acquire));
    }
}

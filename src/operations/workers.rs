//! Admission for blocking request work shared by Operations clones.
use super::errors::Failure;
use anyhow::{Context, Result};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Semaphore, TryAcquireError};

#[derive(Clone)]
pub(super) struct WorkPool {
    scope: &'static str,
    maximum: usize,
    slots: Arc<Semaphore>,
}

impl WorkPool {
    pub fn new(scope: &'static str, maximum: usize) -> Self {
        assert!(maximum > 0 && maximum <= Semaphore::MAX_PERMITS);
        Self {
            scope,
            maximum,
            slots: Arc::new(Semaphore::new(maximum)),
        }
    }

    pub async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|error| -> anyhow::Error {
                match error {
                    TryAcquireError::Closed => {
                        Failure::conflict("operation service is shutting down").into()
                    }
                    TryAcquireError::NoPermits => {
                        crate::model::CapacityExceeded::new(self.scope, self.maximum).into()
                    }
                }
            })?;
        tokio::task::spawn_blocking(move || {
            // Dropping the caller must not free capacity while the disk/SQL
            // unit still runs, or abandon a mutation already in progress.
            let _permit = permit;
            work()
        })
        .await
        .context("operation blocking worker failed")?
    }

    pub fn close(&self) {
        self.slots.close();
    }

    pub async fn drain(&self) {
        while self.slots.available_permits() != self.maximum {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    pub async fn read<T: Send + 'static>(
        &self,
        work: impl FnOnce(ReadInterest) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let interest = ReadInterest {
            abandoned: Arc::new(AtomicBool::new(false)),
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(10),
        };
        let _guard = ReadGuard(interest.abandoned.clone());
        self.run(move || {
            interest.check()?;
            let result = work(interest.clone())?;
            interest.check()?;
            Ok(result)
        })
        .await
    }
}

#[derive(Clone)]
pub(super) struct ReadInterest {
    abandoned: Arc<AtomicBool>,
    deadline: std::time::Instant,
}

impl ReadInterest {
    /// Checkpoints do not preempt one SQLite, filesystem or transform unit.
    pub fn check(&self) -> Result<()> {
        if self.abandoned.load(Ordering::Acquire) {
            return Err(Failure::read_abandoned().into());
        }
        if std::time::Instant::now() >= self.deadline {
            return Err(Failure::read_deadline().into());
        }
        Ok(())
    }
}

struct ReadGuard(Arc<AtomicBool>);
impl Drop for ReadGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[tokio::test]
    async fn dropped_request_retains_capacity_until_owned_worker_exits() {
        let pool = WorkPool::new("test_family", 1);
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, released) = mpsc::channel();
        let running = pool.clone();
        let caller = tokio::spawn(async move {
            running
                .read(move |interest| {
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                    interest.check()
                })
                .await
        });
        ready.await.unwrap();
        caller.abort();
        let _ = caller.await;
        let error = pool.run(|| Ok(())).await.unwrap_err();
        assert_eq!(
            super::super::errors::operation_failure("control", &error)["code"],
            "capacity_exceeded"
        );
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if pool.run(|| Ok(())).await.is_ok() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn abandoned_read_is_observable_at_worker_checkpoints() {
        let pool = WorkPool::new("test_family", 1);
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, released) = mpsc::channel();
        let (observed, result) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(async move {
            pool.read(move |interest| {
                entered.send(()).unwrap();
                released.recv().unwrap();
                let error = interest.check().unwrap_err();
                observed
                    .send(super::super::errors::operation_failure("control", &error))
                    .unwrap();
                Ok(())
            })
            .await
        });
        ready.await.unwrap();
        caller.abort();
        let _ = caller.await;
        release.send(()).unwrap();
        assert_eq!(result.await.unwrap()["code"], "request_abandoned");
    }

    #[tokio::test]
    async fn closing_refuses_new_work_and_drains_the_owned_worker() {
        let pool = WorkPool::new("test_family", 1);
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, released) = mpsc::channel();
        let active = pool.clone();
        let caller = tokio::spawn(async move {
            active
                .run(move || {
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                    Ok(())
                })
                .await
        });
        ready.await.unwrap();
        pool.close();
        let error = pool.run(|| Ok(())).await.unwrap_err();
        assert_eq!(
            super::super::errors::operation_failure("control", &error)["code"],
            "conflict"
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), pool.drain())
                .await
                .is_err()
        );
        release.send(()).unwrap();
        caller.await.unwrap().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), pool.drain())
            .await
            .unwrap();
    }
}

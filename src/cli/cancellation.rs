use anyhow::Result;
use std::{future::Future, pin::Pin};

/// One registered interrupt spans admission, reads and waits. Dropping an
/// admission acknowledgement would lose the exact child that must be canceled.
pub(super) struct CountCancellation<S> {
    signal: Pin<Box<S>>,
    requested: bool,
}

impl<S: Future<Output = Result<()>>> CountCancellation<S> {
    pub(super) fn new(signal: S) -> Self {
        Self {
            signal: Box::pin(signal),
            requested: false,
        }
    }
    pub(super) fn requested(&self) -> bool {
        self.requested
    }
    pub(super) async fn wait<T, W, C, CF>(&mut self, work: W, cancel: C) -> Result<T>
    where
        W: Future<Output = Result<T>>,
        C: FnOnce() -> CF,
        CF: Future<Output = Result<()>>,
    {
        tokio::pin!(work);
        if self.requested {
            return work.await;
        }
        tokio::select! {
            biased;
            signal = self.signal.as_mut() => {
                signal?;
                self.requested = true;
                cancel().await?;
                work.await
            },
            result = &mut work => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CountCancellation;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn interruption_during_admission_waits_for_the_exact_child_handle() {
        let (signal, interrupted) = oneshot::channel();
        let (admit, admission) = oneshot::channel();
        let job = tokio::spawn(async move {
            let mut cancellation = CountCancellation::new(async {
                interrupted.await?;
                Ok(())
            });
            let child = cancellation
                .wait(async { Ok(admission.await?) }, || async { Ok(()) })
                .await
                .unwrap();
            assert!(cancellation.requested());
            child
        });
        signal.send(()).unwrap();
        tokio::task::yield_now().await;
        assert!(
            !job.is_finished(),
            "the acknowledgement must not be abandoned before its child ID is known"
        );
        admit.send("exact-counting-child").unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), job)
                .await
                .unwrap()
                .unwrap(),
            "exact-counting-child"
        );
    }

    #[tokio::test]
    async fn interruption_during_a_query_cancels_once_and_remains_registered_across_waits() {
        let (signal, interrupted) = oneshot::channel();
        let (answer, query) = oneshot::channel();
        let (canceled, observed_cancel) = oneshot::channel();
        let (ready, query_ready) = oneshot::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let job = tokio::spawn(async move {
            let mut cancellation = CountCancellation::new(async {
                interrupted.await?;
                Ok(())
            });
            assert_eq!(
                cancellation
                    .wait(async { Ok("first query") }, || async {
                        panic!("not interrupted")
                    })
                    .await
                    .unwrap(),
                "first query"
            );
            ready.send(()).unwrap();
            let value = cancellation
                .wait(async { Ok(query.await?) }, || async {
                    observed.fetch_add(1, Ordering::SeqCst);
                    canceled.send(()).unwrap();
                    Ok(())
                })
                .await
                .unwrap();
            cancellation
                .wait(async { Ok(()) }, || async {
                    panic!("one interruption must not cancel twice")
                })
                .await
                .unwrap();
            value
        });
        query_ready.await.unwrap();
        signal.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), observed_cancel)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!job.is_finished());
        answer.send("running query response").unwrap();
        assert_eq!(job.await.unwrap(), "running query response");
    }
}

mod applicability;
mod dependencies;
mod engine;
mod files;
mod input;
mod lifecycle;
mod package;
mod process;

pub use engine::Engine;
mod admission;

/// Blocking units retain their caller's admission permit and original deadline.
/// A running unit cannot be preempted by dropping its awaiting future.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    use anyhow::Context;
    tokio::task::spawn_blocking(work)
        .await
        .context("analysis blocking unit failed")?
}

pub(crate) mod applicability;
mod attempt;
mod dependencies;
mod engine;
mod files;
mod finishing;
mod input;
mod lifecycle;
mod output;
mod package;
mod process;
mod retention;

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

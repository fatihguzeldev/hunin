//! Normalization publication and retained-input reanalysis.
mod publication;
mod reanalysis;

pub(crate) use publication::{publish_controlled, source_snapshots};
pub(crate) use reanalysis::{ReanalysisSidecarLimit, reanalyze_controlled};

use anyhow::Result;
use std::time::Instant;

pub(crate) struct Publication {
    pub accepted: bool,
    pub diagnostics: Vec<String>,
}

fn checkpoint(deadline: Option<Instant>) -> Result<()> {
    anyhow::ensure!(
        deadline.is_none_or(|deadline| Instant::now() < deadline),
        "normalization finishing checkpoint deadline elapsed"
    );
    Ok(())
}

#[cfg(test)]
mod tests;

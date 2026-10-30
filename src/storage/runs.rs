//! Recording manifests, bounded pages and import completion updates.
use super::{Store, metadata};
use crate::model::runs::{Run, RunStatus};
use anyhow::{Context, Result};
use rusqlite::{TransactionBehavior, params};

impl Store {
    pub fn save_run(&self, run: &Run) -> Result<()> {
        self.connection()?.execute("INSERT INTO runs(id, started_at_us, manifest) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET manifest=excluded.manifest",params![run.id, run.started_at_us, serde_json::to_string(run)?])?;
        Ok(())
    }

    /// Merge import diagnostics without replacing execution finalization or
    /// recreating a recording that was removed while normalization was running.
    pub fn finish_recording_import(
        &self,
        run_id: &str,
        diagnostics: &[String],
        complete_new_run: bool,
    ) -> Result<Run> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let text: String = tx
            .query_row("SELECT manifest FROM runs WHERE id=?1", [run_id], |r| {
                r.get(0)
            })
            .with_context(|| format!("run removed before import completion: {run_id}"))?;
        let mut run: Run = serde_json::from_str(&text)?;
        anyhow::ensure!(
            run.status != RunStatus::Running,
            "cannot import into a running recording"
        );
        if complete_new_run {
            run.status = RunStatus::Completed;
        }
        run.diagnostics.extend_from_slice(diagnostics);
        tx.execute(
            "UPDATE runs SET manifest=?2 WHERE id=?1",
            params![run_id, serde_json::to_string(&run)?],
        )?;
        tx.commit()?;
        Ok(run)
    }

    pub fn run(&self, id: &str) -> Result<Run> {
        let text: String = self
            .connection()?
            .query_row("SELECT manifest FROM runs WHERE id=?1", [id], |r| r.get(0))
            .with_context(|| format!("run not found: {id}"))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn runs(&self, limit: usize, offset: usize) -> Result<Vec<Run>> {
        self.runs_page(limit.min(500), offset)
    }

    /// Internal pagination includes one sentinel beyond the public 500-run page.
    pub(crate) fn runs_page(&self, limit: usize, offset: usize) -> Result<Vec<Run>> {
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        let values = metadata::read_page(
            &tx,
            "SELECT rowid FROM runs ORDER BY started_at_us DESC,id LIMIT ?1 OFFSET ?2",
            params![limit.min(501), offset],
            "SELECT manifest FROM runs WHERE rowid=?1",
        )?;
        tx.commit()?;
        Ok(values)
    }
}

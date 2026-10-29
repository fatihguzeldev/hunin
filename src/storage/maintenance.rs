use super::Store;
use crate::model::runs::{Artifact, RunStatus};
use anyhow::{Context, Result, bail};
use rusqlite::TransactionBehavior;
use std::{fs, path::Path};

impl Store {
    pub fn remove_run(&self, run_id: &str) -> Result<()> {
        if self.run(run_id)?.status == RunStatus::Running {
            bail!("cancel a running recording before removing it");
        }
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidates = tx
            .prepare("SELECT metadata FROM artifacts WHERE run_id=?1")?
            .query_map([run_id], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str::<Artifact>(&r?)?))
            .collect::<Result<Vec<_>>>()?;
        if tx.query_row("SELECT COUNT(*) FROM documents WHERE owner_run_id=?1 AND json_extract(payload,'$.status')='running'",[run_id],|r|r.get::<_,i64>(0))?>0 {bail!("cancel active analyzer executions before removing this recording");}
        let attempts: Vec<String> = tx
            .prepare(
                "SELECT id FROM documents WHERE namespace='analysis-results' AND owner_run_id=?1",
            )?
            .query_map([run_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        tx.execute("DELETE FROM runs WHERE id=?1", [run_id])?;
        tx.commit()?;
        // Only collect after deleting metadata commits. Artifact writers use the
        // same write lock, so a concurrent new reference cannot lose its bytes.
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for a in candidates {
            let referenced: i64 = tx.query_row(
                "SELECT COUNT(*) FROM artifacts WHERE sha256=?1",
                [&a.sha256],
                |r| r.get(0),
            )?;
            if referenced == 0 {
                let path = self.root.join("artifacts").join(a.sha256);
                if path.exists() {
                    fs::remove_file(path)?;
                }
            }
        }
        tx.commit()?;
        remove_managed_directory(&self.root.join("captures"), run_id)?;
        for id in attempts {
            remove_managed_directory(&self.root.join("analysis-executions"), &id)?;
        }
        Ok(())
    }
}

fn remove_managed_directory(root: &Path, id: &str) -> Result<()> {
    // Imported IDs are database keys, never permission to traverse a filesystem.
    let mut components = Path::new(id).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Ok(());
    }
    let path = root.join(id);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .context("recording metadata removed; managed directory cleanup failed");
        }
    };
    if metadata.is_symlink() || !metadata.is_dir() {
        fs::remove_file(&path)
    } else {
        fs::remove_dir_all(&path)
    }
    .with_context(|| {
        format!(
            "recording metadata removed; could not clean managed path {}",
            path.display()
        )
    })
}

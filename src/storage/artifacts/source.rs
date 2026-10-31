//! Stage bounded source bytes before publishing all references in one transaction.
use super::*;

pub(crate) struct SourceArtifactBatch {
    store: Store,
    run_id: String,
    pending: Vec<(Artifact, PendingArtifact)>,
}

impl Store {
    pub(crate) fn source_artifact_batch(&self, run_id: &str) -> Result<SourceArtifactBatch> {
        // Validate once, rather than decoding the large run manifest per file.
        self.run(run_id)?;
        Ok(SourceArtifactBatch {
            store: self.clone(),
            run_id: run_id.into(),
            pending: Vec::new(),
        })
    }
}

impl SourceArtifactBatch {
    pub(crate) fn stage(&mut self, name: &str, bytes: &[u8]) -> Result<Artifact> {
        validate_artifact_name(name)?;
        let pending = self.store.stage_artifact(
            &mut std::io::Cursor::new(bytes),
            "source-snapshot",
            None,
            &mut 0,
            &mut || Ok(()),
        )?;
        let artifact = artifact_reference(name, "source-snapshot", &pending)?;
        self.pending.push((artifact.clone(), pending));
        Ok(artifact)
    }

    pub(crate) fn publish(self) -> Result<()> {
        // Connection identity and the run's continued existence are checked at
        // publication. Staging never makes partial source references visible.
        let mut connection = self.store.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1)",
            [&self.run_id],
            |r| r.get(0),
        )?;
        anyhow::ensure!(exists, "run was removed before source artifact publication");
        for (artifact, pending) in &self.pending {
            let destination = self.store.root.join("artifacts").join(&artifact.sha256);
            if !destination.exists() {
                fs::rename(&pending.path, &destination)?;
            } else if !same_artifact_contents(&pending.path, &destination, &mut || Ok(()))? {
                bail!("artifact identity mismatch");
            }
            tx.execute(
                "INSERT OR IGNORE INTO artifacts(run_id,id,metadata,sha256) VALUES(?1,?2,?3,?4)",
                params![
                    self.run_id,
                    artifact.id,
                    serde_json::to_string(artifact)?,
                    artifact.sha256
                ],
            )?;
        }
        // Each staged file was synced before closing. Persist all renamed
        // directory entries before the single reference-publication commit.
        #[cfg(unix)]
        if let Ok(directory) = fs::File::open(self.store.root.join("artifacts")) {
            let _ = directory.sync_all();
        }
        tx.commit()?;
        Ok(())
    }
}

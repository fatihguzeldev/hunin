use crate::{
    ingestion,
    model::runs::{Artifact, Run},
};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root.join("artifacts"))?;
        let store = Self {
            root: root.canonicalize()?,
        };
        store
            .connection()?
            .execute_batch(include_str!("storage/schema.sql"))?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn connection(&self) -> Result<Connection> {
        let c = Connection::open(self.root.join("hunin.sqlite"))?;
        c.busy_timeout(Duration::from_secs(10))?;
        c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        Ok(c)
    }

    pub fn save_run(&self, run: &Run) -> Result<()> {
        self.connection()?.execute("INSERT INTO runs(id, started_at_us, manifest) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET manifest=excluded.manifest",params![run.id, run.started_at_us, serde_json::to_string(run)?])?;
        Ok(())
    }

    pub fn run(&self, id: &str) -> Result<Run> {
        let text: String = self
            .connection()?
            .query_row("SELECT manifest FROM runs WHERE id=?1", [id], |r| r.get(0))
            .with_context(|| format!("run not found: {id}"))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn runs(&self, limit: usize, offset: usize) -> Result<Vec<Run>> {
        let c = self.connection()?;
        let mut statement = c.prepare(
            "SELECT manifest FROM runs ORDER BY started_at_us DESC,id LIMIT ?1 OFFSET ?2",
        )?;
        let rows =
            statement.query_map(params![limit.min(500), offset], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    /// Validate and publish a whole result set in one transaction. Replays are no-ops.
    pub fn accept(&self, run_id: &str, input_identity: &str, records: &[Value]) -> Result<bool> {
        self.run(run_id)?;
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if tx
            .query_row(
                "SELECT 1 FROM result_sets WHERE run_id=?1 AND input_identity=?2",
                params![run_id, input_identity],
                |r| r.get::<_, i32>(0),
            )
            .optional()?
            .is_some()
        {
            return Ok(false);
        }
        let existing: HashSet<String> = tx
            .prepare("SELECT id FROM observations WHERE run_id=?1")?
            .query_map([run_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        ingestion::validate(run_id, records, &existing)?;
        let result_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO result_sets(id,run_id,input_identity) VALUES(?1,?2,?3)",
            params![result_id, run_id, input_identity],
        )?;
        {
            let mut insert=tx.prepare("INSERT INTO observations(run_id,id,kind,timestamp_us,result_set_id,record) VALUES(?1,?2,?3,?4,?5,?6)")?;
            for r in records {
                insert.execute(params![
                    run_id,
                    r["id"].as_str(),
                    r["kind"].as_str(),
                    r["timestamp_us"].as_u64(),
                    result_id,
                    serde_json::to_string(r)?
                ])?;
            }
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn records(&self, run_id: &str) -> Result<Vec<Value>> {
        self.run(run_id)?;
        let c = self.connection()?;
        let mut statement =
            c.prepare("SELECT record FROM observations WHERE run_id=?1 ORDER BY timestamp_us,seq")?;
        statement
            .query_map([run_id], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }

    pub fn add_artifact(&self, run_id: &str, source: &Path, format: &str) -> Result<Artifact> {
        self.run(run_id)?;
        let bytes =
            fs::read(source).with_context(|| format!("read artifact {}", source.display()))?;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let dest = self.root.join("artifacts").join(&sha256);
        // An atomic rename publishes complete bytes; identical concurrent writers agree.
        if !dest.exists() {
            let temp = dest.with_extension(uuid::Uuid::new_v4().to_string());
            fs::write(&temp, &bytes)?;
            fs::rename(&temp, &dest)?;
        } else if fs::read(&dest)? != bytes {
            bail!("artifact identity mismatch");
        }
        let artifact = Artifact {
            id: sha256.clone(),
            name: source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            size: bytes.len() as u64,
            sha256,
            format: format.into(),
        };
        tx.execute(
            "INSERT OR IGNORE INTO artifacts(run_id,id,metadata) VALUES(?1,?2,?3)",
            params![run_id, artifact.id, serde_json::to_string(&artifact)?],
        )?;
        tx.commit()?;
        Ok(artifact)
    }

    pub fn artifacts(&self, run_id: &str) -> Result<Vec<Artifact>> {
        self.run(run_id)?;
        let c = self.connection()?;
        let mut statement =
            c.prepare("SELECT metadata FROM artifacts WHERE run_id=?1 ORDER BY id")?;
        statement
            .query_map([run_id], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }

    pub fn artifact_path(&self, run_id: &str, id: &str) -> Result<PathBuf> {
        // Resolve through a run-owned reference, never by joining an arbitrary caller path.
        let artifact = self
            .artifacts(run_id)?
            .into_iter()
            .find(|a| a.id == id)
            .context("artifact not found")?;
        Ok(self.root.join("artifacts").join(artifact.sha256))
    }

    pub fn remove_run(&self, run_id: &str) -> Result<()> {
        if self.run(run_id)?.status == "running" {
            bail!("cancel a running recording before removing it");
        }
        let candidates = self.artifacts(run_id)?;
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM runs WHERE id=?1", [run_id])?;
        tx.commit()?;
        // Only collect after deleting metadata commits. Artifact writers use the
        // same write lock, so a concurrent new reference cannot lose its bytes.
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for a in candidates {
            let referenced: i64 =
                tx.query_row("SELECT COUNT(*) FROM artifacts WHERE id=?1", [&a.id], |r| {
                    r.get(0)
                })?;
            if referenced == 0 {
                let path = self.root.join("artifacts").join(a.sha256);
                if path.exists() {
                    fs::remove_file(path)?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
}

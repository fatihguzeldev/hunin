use crate::{
    ingestion,
    model::runs::{Artifact, Run},
};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn document(&self,namespace:&str,id:&str)->Result<Value> {
        let text:String=self.connection()?.query_row("SELECT payload FROM documents WHERE namespace=?1 AND id=?2",params![namespace,id],|r|r.get(0)).context("document not found")?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn documents(&self,namespace:&str)->Result<Vec<Value>> {
        let c=self.connection()?;
        let mut statement=c.prepare("SELECT payload FROM documents WHERE namespace=?1 ORDER BY updated_at_us DESC,id")?;
        statement.query_map([namespace],|r|r.get::<_,String>(0))?.map(|r|Ok(serde_json::from_str(&r?)?)).collect()
    }

    pub fn save_document(&self,namespace:&str,id:&str,value:&Value)->Result<()> {
        self.connection()?.execute("INSERT INTO documents(namespace,id,payload,updated_at_us) VALUES(?1,?2,?3,?4) ON CONFLICT(namespace,id) DO UPDATE SET payload=excluded.payload,updated_at_us=excluded.updated_at_us",params![namespace,id,serde_json::to_string(value)?,crate::model::now_us()])?;Ok(())
    }
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root.join("artifacts"))?;
        let store = Self {
            root: root.canonicalize()?,
        };
        store
            .connection()?
            .execute_batch(include_str!("storage/schema.sql"))?;
        let c = store.connection()?;
        let columns: Vec<String> = c
            .prepare("PRAGMA table_info(artifacts)")?
            .query_map([], |r| r.get(1))?
            .collect::<rusqlite::Result<_>>()?;
        if !columns.iter().any(|name| name == "sha256") {
            c.execute_batch("ALTER TABLE artifacts ADD COLUMN sha256 TEXT; UPDATE artifacts SET sha256=json_extract(metadata,'$.sha256');")?;
        }
        c.execute_batch(
            "CREATE INDEX IF NOT EXISTS artifact_blob ON artifacts(sha256); PRAGMA user_version=1;",
        )?;
        let columns:Vec<String>=c.prepare("PRAGMA table_info(result_sets)")?.query_map([],|r|r.get(1))?.collect::<rusqlite::Result<_>>()?;
        if !columns.iter().any(|name|name=="source_key") {c.execute_batch("ALTER TABLE result_sets ADD COLUMN source_key TEXT NOT NULL DEFAULT ''; UPDATE result_sets SET source_key=input_identity;")?;}
        if !columns.iter().any(|name|name=="active") {c.execute_batch("ALTER TABLE result_sets ADD COLUMN active INTEGER NOT NULL DEFAULT 1;")?;}
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
        self.publish(run_id,input_identity,input_identity,records,None)
    }

    pub fn accept_import(&self,run_id:&str,source_key:&str,fingerprint:&str,records:&[Value])->Result<bool> {
        self.publish(run_id,source_key,&format!("normalization:{fingerprint}"),records,None)
    }

    pub fn publish(&self,run_id:&str,source_key:&str,input_identity:&str,records:&[Value],document:Option<(&str,&str,&Value)>)->Result<bool> {
        self.run(run_id)?;
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((id,active))=tx.query_row("SELECT id,active FROM result_sets WHERE run_id=?1 AND input_identity=?2",params![run_id,input_identity],|r|Ok((r.get::<_,String>(0)?,r.get::<_,bool>(1)?))).optional()? {
            if !active {
                tx.execute("UPDATE result_sets SET active=0 WHERE run_id=?1 AND source_key=?2",params![run_id,source_key])?;
                tx.execute("UPDATE result_sets SET active=1 WHERE id=?1",[id])?;
                tx.commit()?;
            }
            return Ok(false);
        }
        let replacing=tx.query_row("SELECT COUNT(*) FROM result_sets WHERE run_id=?1 AND source_key=?2 AND active=1",params![run_id,source_key],|r|r.get::<_,i64>(0))?>0;
        tx.execute("UPDATE result_sets SET active=0 WHERE run_id=?1 AND source_key=?2",params![run_id,source_key])?;
        if replacing && source_key.starts_with("import-v1:") {tx.execute("UPDATE result_sets SET active=0 WHERE run_id=?1 AND source_key LIKE 'analysis:%'",[run_id])?;}
        let existing: HashSet<String> = tx
            .prepare("SELECT o.id FROM observations o JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND r.active=1")?
            .query_map([run_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        ingestion::validate(run_id, records, &existing)?;
        let existing_rows: HashMap<String, Value> = tx
            .prepare("SELECT o.id,o.record FROM observations o JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND r.active=1")?
            .query_map([run_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .map(|row| {
                let (id, text) = row?;
                Ok((id, serde_json::from_str(&text)?))
            })
            .collect::<Result<_>>()?;
        ingestion::validate_ownership(records, &existing_rows)?;
        let result_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO result_sets(id,run_id,input_identity,source_key,active) VALUES(?1,?2,?3,?4,1)",
            params![result_id, run_id, input_identity,source_key],
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
        if let Some((namespace,id,value))=document {tx.execute("INSERT INTO documents(namespace,id,payload,updated_at_us) VALUES(?1,?2,?3,?4) ON CONFLICT(namespace,id) DO UPDATE SET payload=excluded.payload,updated_at_us=excluded.updated_at_us",params![namespace,id,serde_json::to_string(value)?,crate::model::now_us()])?;}
        tx.commit()?;
        Ok(true)
    }

    pub fn records(&self, run_id: &str) -> Result<Vec<Value>> {
        self.run(run_id)?;
        let c = self.connection()?;
        let mut statement = c.prepare("SELECT o.record FROM observations o JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND r.active=1 ORDER BY o.timestamp_us,o.seq")?;
        statement
            .query_map([run_id], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }

    pub fn records_history(&self,run_id:&str)->Result<Vec<Value>> {
        self.run(run_id)?;let c=self.connection()?;
        let mut statement=c.prepare("SELECT record FROM observations WHERE run_id=?1 ORDER BY seq")?;
        statement.query_map([run_id],|r|r.get::<_,String>(0))?.map(|r|Ok(serde_json::from_str(&r?)?)).collect()
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
        let name = source
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let reference_id = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(&sha256, &name, format))?)
        );
        let artifact = Artifact {
            id: reference_id,
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
            "INSERT OR IGNORE INTO artifacts(run_id,id,metadata,sha256) VALUES(?1,?2,?3,?4)",
            params![
                run_id,
                artifact.id,
                serde_json::to_string(&artifact)?,
                artifact.sha256
            ],
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
        Ok(())
    }
}

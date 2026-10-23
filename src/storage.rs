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
    pub fn document(&self, namespace: &str, id: &str) -> Result<Value> {
        let text: String = self
            .connection()?
            .query_row(
                "SELECT payload FROM documents WHERE namespace=?1 AND id=?2",
                params![namespace, id],
                |r| r.get(0),
            )
            .context("document not found")?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn documents(&self, namespace: &str) -> Result<Vec<Value>> {
        let c = self.connection()?;
        let mut statement = c.prepare(
            "SELECT payload FROM documents WHERE namespace=?1 ORDER BY updated_at_us DESC,id",
        )?;
        statement
            .query_map([namespace], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }

    pub fn save_document(&self, namespace: &str, id: &str, value: &Value) -> Result<()> {
        write_document(&self.connection()?, namespace, id, value)
    }

    /// Persist a cross-client cancellation without overwriting a completed attempt.
    pub fn request_analysis_cancel(&self, id: &str) -> Result<Value> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let text: String = tx
            .query_row(
                "SELECT payload FROM documents WHERE namespace='analysis-results' AND id=?1",
                [id],
                |r| r.get(0),
            )
            .context("analyzer execution not found")?;
        let mut value: Value = serde_json::from_str(&text)?;
        let running = value["status"] == "running";
        if running {
            value["cancellation_requested"] = Value::Bool(true);
            write_document(&tx, "analysis-results", id, &value)?;
        }
        tx.commit()?;
        Ok(value)
    }
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root.join("artifacts"))?;
        let store = Self {
            root: root.canonicalize()?,
        };
        let mut connection = store.connection()?;
        let version: u32 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        anyhow::ensure!(
            version <= 2,
            "recording database schema {version} requires a newer Hunin; this executable supports up to 2"
        );
        let c = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        c.execute_batch(include_str!("storage/schema.sql"))?;
        let columns: Vec<String> = c
            .prepare("PRAGMA table_info(artifacts)")?
            .query_map([], |r| r.get(1))?
            .collect::<rusqlite::Result<_>>()?;
        if !columns.iter().any(|name| name == "sha256") {
            c.execute_batch("ALTER TABLE artifacts ADD COLUMN sha256 TEXT; UPDATE artifacts SET sha256=json_extract(metadata,'$.sha256');")?;
        }
        c.execute_batch("CREATE INDEX IF NOT EXISTS artifact_blob ON artifacts(sha256);")?;
        let columns: Vec<String> = c
            .prepare("PRAGMA table_info(result_sets)")?
            .query_map([], |r| r.get(1))?
            .collect::<rusqlite::Result<_>>()?;
        if !columns.iter().any(|name| name == "source_key") {
            c.execute_batch("ALTER TABLE result_sets ADD COLUMN source_key TEXT NOT NULL DEFAULT ''; UPDATE result_sets SET source_key=input_identity;")?;
        }
        if !columns.iter().any(|name| name == "active") {
            c.execute_batch(
                "ALTER TABLE result_sets ADD COLUMN active INTEGER NOT NULL DEFAULT 1;",
            )?;
        }
        let columns: Vec<String> = c
            .prepare("PRAGMA table_info(documents)")?
            .query_map([], |r| r.get(1))?
            .collect::<rusqlite::Result<_>>()?;
        if !columns.iter().any(|name| name == "owner_run_id") {
            c.execute_batch("ALTER TABLE documents ADD COLUMN owner_run_id TEXT REFERENCES runs(id) ON DELETE CASCADE;")?;
        }
        c.execute_batch("UPDATE documents SET owner_run_id=json_extract(payload,'$.run_id') WHERE namespace IN('analysis-results','cancellations') AND owner_run_id IS NULL AND json_extract(payload,'$.run_id') IN(SELECT id FROM runs); PRAGMA user_version=2;")?;
        c.commit()?;
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
        self.publish(run_id, input_identity, input_identity, records, None)
    }

    pub fn accept_import(
        &self,
        run_id: &str,
        source_key: &str,
        fingerprint: &str,
        records: &[Value],
    ) -> Result<bool> {
        self.publish(
            run_id,
            source_key,
            &format!("normalization:{fingerprint}"),
            records,
            None,
        )
    }

    pub fn publish(
        &self,
        run_id: &str,
        source_key: &str,
        input_identity: &str,
        records: &[Value],
        document: Option<(&str, &str, &Value)>,
    ) -> Result<bool> {
        self.run(run_id)?;
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(("analysis-results", id, _)) = document {
            let cancelled: Option<bool> = tx.query_row("SELECT COALESCE(json_extract(payload,'$.cancellation_requested'),0) FROM documents WHERE namespace='analysis-results' AND id=?1",[id],|r|r.get(0)).optional()?;
            anyhow::ensure!(
                cancelled != Some(true),
                "analyzer execution was cancelled before publication"
            );
        }
        if let Some((id, active, previous_source)) = tx
            .query_row(
                "SELECT id,active,source_key FROM result_sets WHERE run_id=?1 AND input_identity=?2",
                params![run_id, input_identity],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, String>(2)?)),
            )
            .optional()?
        {
            anyhow::ensure!(previous_source == source_key, "input identity belongs to another result source");
            if !active {
                deactivate_source(&tx, run_id, source_key)?;
                let stale: i64 = tx.query_row("SELECT COUNT(*) FROM result_set_dependencies d JOIN result_sets r ON r.id=d.input_result_set_id WHERE d.result_set_id=?1 AND r.active=0", [&id], |r|r.get(0))?;
                anyhow::ensure!(stale == 0, "stored result depends on superseded inputs; recompute the analysis");
                tx.execute("UPDATE result_sets SET active=1 WHERE id=?1", [&id])?;
            }
            if let Some((namespace, key, value)) = document {
                let mut value = value.clone();
                value["status"] = serde_json::json!("replayed");
                value["result_set_id"] = serde_json::json!(id);
                write_document(&tx, namespace, key, &value)?;
            }
            tx.commit()?;
            return Ok(false);
        }
        deactivate_source(&tx, run_id, source_key)?;
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
        let mut dependencies = HashSet::new();
        let own_ids: HashSet<&str> = records.iter().filter_map(|r| r["id"].as_str()).collect();
        for record in records {
            let references = ["parent_id", "context_id", "from", "to"]
                .into_iter()
                .filter_map(|key| record[key].as_str())
                .chain(
                    record["evidence"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str),
                );
            for reference in references.filter(|id| !own_ids.contains(id)) {
                let id: String = tx.query_row("SELECT o.result_set_id FROM observations o JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND o.id=?2 AND r.active=1",params![run_id,reference],|r|r.get(0))?;
                dependencies.insert(id);
            }
        }
        if let Some((_, _, value)) = document {
            if let Some(inputs) = value.get("input_result_sets") {
                for id in inputs
                    .as_array()
                    .context("input_result_sets must be an array")?
                {
                    dependencies.insert(
                        id.as_str()
                            .context("input result identity must be a string")?
                            .to_owned(),
                    );
                }
            }
        }
        for id in &dependencies {
            let selected: i64 = tx.query_row(
                "SELECT COUNT(*) FROM result_sets WHERE id=?1 AND run_id=?2 AND active=1",
                params![id, run_id],
                |r| r.get(0),
            )?;
            anyhow::ensure!(
                selected == 1,
                "analysis input changed before publication; rerun with current selected results"
            );
        }
        let result_id = document
            .filter(|(ns, _, _)| *ns == "analysis-results")
            .map(|(_, id, _)| id.to_owned())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        tx.execute(
            "INSERT INTO result_sets(id,run_id,input_identity,source_key,active) VALUES(?1,?2,?3,?4,1)",
            params![result_id, run_id, input_identity,source_key],
        )?;
        for id in dependencies {
            tx.execute("INSERT INTO result_set_dependencies(result_set_id,input_result_set_id) VALUES(?1,?2)",params![result_id,id])?;
        }
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
        if let Some((namespace, id, value)) = document {
            write_document(&tx, namespace, id, value)?;
        }
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

    pub fn records_history(&self, run_id: &str) -> Result<Vec<Value>> {
        self.run(run_id)?;
        let c = self.connection()?;
        let mut statement =
            c.prepare("SELECT record FROM observations WHERE run_id=?1 ORDER BY seq")?;
        statement
            .query_map([run_id], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }

    /// Snapshot candidates omit the analyzer's old results and their dependents.
    pub fn active_result_ids(&self, run_id: &str, exclude_source_key: &str) -> Result<Vec<String>> {
        self.run(run_id)?;
        let c = self.connection()?;
        let mut query = c.prepare("WITH RECURSIVE omitted(id) AS (SELECT id FROM result_sets WHERE run_id=?1 AND source_key=?2 UNION SELECT d.result_set_id FROM result_set_dependencies d JOIN omitted o ON d.input_result_set_id=o.id) SELECT id FROM result_sets WHERE run_id=?1 AND active=1 AND id NOT IN(SELECT id FROM omitted) ORDER BY id")?;
        Ok(query
            .query_map(params![run_id, exclude_source_key], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn records_for_result_ids(&self, run_id: &str, ids: &[String]) -> Result<Vec<Value>> {
        self.run(run_id)?;
        let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let c = self.connection()?;
        let mut query = c.prepare("SELECT o.result_set_id,o.record FROM observations o JOIN result_sets r ON r.id=o.result_set_id WHERE o.run_id=?1 AND r.active=1 ORDER BY o.timestamp_us,o.seq")?;
        let mut result = Vec::new();
        for row in query.query_map([run_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (id, record) = row?;
            if wanted.contains(id.as_str()) {
                result.push(serde_json::from_str(&record)?);
            }
        }
        Ok(result)
    }

    pub fn selected_result_id(&self, run_id: &str, source_key: &str) -> Result<Option<String>> {
        Ok(self
            .connection()?
            .query_row(
                "SELECT id FROM result_sets WHERE run_id=?1 AND source_key=?2 AND active=1",
                params![run_id, source_key],
                |r| r.get(0),
            )
            .optional()?)
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

fn write_document(connection: &Connection, namespace: &str, id: &str, value: &Value) -> Result<()> {
    let owner = if matches!(namespace, "analysis-results" | "cancellations") {
        Some(
            value["run_id"]
                .as_str()
                .context("analyzer result requires an owning run")?,
        )
    } else {
        None
    };
    connection.execute("INSERT INTO documents(namespace,id,payload,updated_at_us,owner_run_id) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(namespace,id) DO UPDATE SET payload=excluded.payload,updated_at_us=excluded.updated_at_us,owner_run_id=excluded.owner_run_id",params![namespace,id,serde_json::to_string(value)?,crate::model::now_us(),owner])?;
    Ok(())
}

fn deactivate_source(connection: &Connection, run_id: &str, source_key: &str) -> Result<()> {
    connection.execute("WITH RECURSIVE stale(id) AS (SELECT id FROM result_sets WHERE run_id=?1 AND source_key=?2 AND active=1 UNION SELECT d.result_set_id FROM result_set_dependencies d JOIN stale s ON d.input_result_set_id=s.id) UPDATE result_sets SET active=0 WHERE id IN(SELECT id FROM stale)",params![run_id,source_key])?;
    Ok(())
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

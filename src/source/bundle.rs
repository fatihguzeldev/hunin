//! Versioned retained-source ranges inside one immutable, content-addressed blob.
use crate::{model::runs::Run, storage::Store};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, time::Instant};

pub(super) const FORMAT: &str = "source-bundle-v1";
pub(super) const MAX_CAPTURE_BYTES: u64 = 32 * 1024 * 1024;
const CHUNK_BYTES: usize = 4 * 1024 * 1024;
const FILE_BYTES: usize = 2 * 1024 * 1024;

pub(super) struct Index {
    pub snapshots: Vec<Value>,
    pub bundles: Vec<Value>,
    pub publication_us: u64,
}

/// Chunk at file boundaries: bounded memory and reusable unchanged groups.
pub(super) struct Writer<'a> {
    store: &'a Store,
    run_id: &'a str,
    content: Vec<u8>,
    pending_start: usize,
    index: Index,
}

impl<'a> Writer<'a> {
    pub fn new(store: &'a Store, run_id: &'a str) -> Self {
        Self {
            store,
            run_id,
            content: Vec::new(),
            pending_start: 0,
            index: Index {
                snapshots: Vec::new(),
                bundles: Vec::new(),
                publication_us: 0,
            },
        }
    }
    pub fn push(&mut self, path: &str, content: &[u8]) -> Result<()> {
        ensure!(
            content.len() <= FILE_BYTES,
            "source file exceeds bundle limit"
        );
        if self.content.len() + content.len() > CHUNK_BYTES {
            self.flush()?;
        }
        self.index.snapshots.push(json!({"path":path,"sha256":format!("{:x}", Sha256::digest(content)),"offset":self.content.len(),"bytes":content.len()}));
        self.content.extend_from_slice(content);
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        if self.pending_start == self.index.snapshots.len() {
            return Ok(());
        }
        let started = Instant::now();
        let artifact =
            self.store
                .add_artifact_bytes(self.run_id, "sources.bundle", FORMAT, &self.content)?;
        self.index.publication_us += started.elapsed().as_micros() as u64;
        for snapshot in &mut self.index.snapshots[self.pending_start..] {
            snapshot["artifact_id"] = json!(artifact.id);
        }
        self.index.bundles.push(json!({"format":FORMAT,"artifact_id":artifact.id,"sha256":artifact.sha256,"bytes":artifact.size}));
        self.pending_start = self.index.snapshots.len();
        self.content.clear();
        Ok(())
    }
    pub fn finish(mut self) -> Result<Index> {
        self.flush()?;
        Ok(self.index)
    }
}

pub(super) fn read(store: &Store, run: &Run, entry: &Value) -> Result<Vec<u8>> {
    let id = entry["artifact_id"]
        .as_str()
        .context("snapshot artifact identity missing")?;
    let path = store.artifact_path(&run.id, id)?;
    let packed = entry.get("offset").is_some();
    let maximum = if packed {
        CHUNK_BYTES as u64
    } else {
        FILE_BYTES as u64
    };
    let mut content = Vec::new();
    fs::File::open(path)?
        .take(maximum + 1)
        .read_to_end(&mut content)?;
    ensure!(
        content.len() as u64 <= maximum,
        "retained source exceeds its byte limit"
    );
    if packed {
        let bundle = run.source["source_bundles"]
            .as_array()
            .and_then(|bundles| bundles.iter().find(|bundle| bundle["artifact_id"] == id))
            .context("source bundle identity missing")?;
        ensure!(
            bundle["format"] == FORMAT,
            "source bundle identity mismatch"
        );
        ensure!(
            bundle["sha256"].as_str() == Some(format!("{:x}", Sha256::digest(&content)).as_str()),
            "source bundle integrity check failed"
        );
        ensure!(
            bundle["bytes"].as_u64() == Some(content.len() as u64),
            "source bundle size mismatch"
        );
        let start = entry["offset"]
            .as_u64()
            .context("source bundle offset missing")?;
        let length = entry["bytes"]
            .as_u64()
            .context("source bundle length missing")?;
        ensure!(
            length <= 2 * 1024 * 1024,
            "retained source exceeds its byte limit"
        );
        let end = start
            .checked_add(length)
            .context("source bundle range overflow")?;
        ensure!(
            end <= content.len() as u64,
            "source bundle range exceeds retained bytes"
        );
        content = content[start as usize..end as usize].to_vec();
    }
    ensure!(
        entry["sha256"].as_str() == Some(format!("{:x}", Sha256::digest(&content)).as_str()),
        "retained source integrity check failed"
    );
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Store, Run) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("store")).unwrap();
        let run: Run = serde_json::from_value(json!({"id":"bundled","name":"source","status":"completed","command":[],"cwd":root.path(),"runtime":"none","started_at_us":0,"finished_at_us":1,"exit_code":0,"diagnostics":[],"source":{},"correctness":{}})).unwrap();
        store.save_run(&run).unwrap();
        (root, store, run)
    }

    #[test]
    fn source_chunks_keep_file_boundaries_and_verify_each_immutable_file() {
        let (_root, store, mut run) = fixture();
        let mut writer = Writer::new(&store, &run.id);
        let content = vec![b'a'; FILE_BYTES];
        for name in ["one.rs", "two.rs", "three.rs"] {
            writer.push(name, &content).unwrap();
        }
        let index = writer.finish().unwrap();
        assert_eq!(index.bundles.len(), 2);
        assert!(
            index
                .bundles
                .iter()
                .all(|b| b["bytes"].as_u64().unwrap() <= CHUNK_BYTES as u64)
        );
        run.source["source_bundles"] = json!(index.bundles);
        for entry in &index.snapshots {
            assert_eq!(read(&store, &run, entry).unwrap(), content);
        }
        let mut invalid = index.snapshots[0].clone();
        invalid["offset"] = json!(u64::MAX);
        assert!(
            read(&store, &run, &invalid)
                .unwrap_err()
                .to_string()
                .contains("overflow")
        );
        invalid = index.snapshots[0].clone();
        invalid["sha256"] = json!("wrong file identity");
        assert!(
            read(&store, &run, &invalid)
                .unwrap_err()
                .to_string()
                .contains("integrity")
        );
        let artifact = store
            .artifact_path(&run.id, index.snapshots[0]["artifact_id"].as_str().unwrap())
            .unwrap();
        fs::write(artifact, b"corrupt retained blob").unwrap();
        assert!(
            read(&store, &run, &index.snapshots[0])
                .unwrap_err()
                .to_string()
                .contains("integrity")
        );
    }

    #[test]
    fn bundled_source_windows_survive_current_file_changes() {
        let (_root, store, mut run) = fixture();
        let mut writer = Writer::new(&store, &run.id);
        writer.push("one.rs", b"first\nsecond\nthird\n").unwrap();
        writer.push("two.rs", b"unrelated\n").unwrap();
        let index = writer.finish().unwrap();
        run.source = json!({"retained_sources":index.snapshots,"source_bundles":index.bundles});
        store.save_run(&run).unwrap();
        fs::write(
            std::path::Path::new(&run.cwd).join("one.rs"),
            b"changed source",
        )
        .unwrap();
        let value = crate::source::read(
            &store,
            &json!({"run_id":run.id,"path":"one.rs","line":2,"count":1}),
        )
        .unwrap();
        assert_eq!(value["content"], "second");
        assert_eq!(value["next_line"], 3);
        assert_eq!(value["total_lines"], 3);
        assert_eq!(value["identity_verified"], true);
    }
}

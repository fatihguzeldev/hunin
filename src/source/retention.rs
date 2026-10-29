//! Retain only bounded, identity-matching source bytes before target launch.
use crate::{model::runs::Run, storage::Store};
use anyhow::Result;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    path::Path,
};

#[derive(Clone, Copy, Serialize)]
struct Limits {
    per_file_bytes: u64,
    total_read_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            per_file_bytes: 2 * 1024 * 1024,
            total_read_bytes: 32 * 1024 * 1024,
        }
    }
}
#[derive(Default, Serialize)]
struct Skips {
    nonregular: usize,
    outside_root: usize,
    unavailable: usize,
    oversized: usize,
    read_budget: usize,
    non_utf8: usize,
    identity_changed: usize,
}
#[derive(Default, Serialize)]
struct Reads {
    bytes: u64,
    skips: Skips,
}

pub(crate) fn retain(store: &Store, run: &mut Run) -> Result<()> {
    retain_with_limits(store, run, Limits::default())
}

fn retain_with_limits(store: &Store, run: &mut Run, limits: Limits) -> Result<()> {
    let root = Path::new(&run.cwd);
    let mut snapshots = Vec::new();
    let mut skipped = 0;
    let mut bytes = 0;
    let mut reads = Reads::default();
    for (relative, expected) in run.source["source_files"].as_object().into_iter().flatten() {
        let path = root.join(relative);
        let reason = match retain_file(
            store,
            &run.id,
            root,
            &path,
            expected.as_str(),
            limits,
            &mut reads,
        )? {
            Ok(artifact) => {
                bytes += artifact.size;
                snapshots.push(
                    json!({"path":relative,"artifact_id":artifact.id,"sha256":artifact.sha256}),
                );
                continue;
            }
            Err(reason) => reason,
        };
        skipped += 1;
        match reason {
            Skip::Nonregular => reads.skips.nonregular += 1,
            Skip::OutsideRoot => reads.skips.outside_root += 1,
            Skip::Unavailable => reads.skips.unavailable += 1,
            Skip::Oversized => reads.skips.oversized += 1,
            Skip::ReadBudget => reads.skips.read_budget += 1,
            Skip::NonUtf8 => reads.skips.non_utf8 += 1,
            Skip::IdentityChanged => reads.skips.identity_changed += 1,
        }
    }
    run.source["retained_sources"] = json!(snapshots);
    run.source["source_retention"] = json!({"bytes":bytes,"skipped_files":skipped,"scope":"pre-launch UTF-8 project files, at most 2 MiB per file and 32 MiB per recording","build_mapping_verified":false,
        "read_budget":{"name":"source-retention-read-v1","limits":limits,"work":reads,"scope":"all attempted source content reads including mismatches; at most one sentinel byte beyond the remaining read budget","limitations":"Static nonregular files are refused; path replacement races are not an adversarial filesystem guarantee"}});
    Ok(())
}

enum Skip {
    Nonregular,
    OutsideRoot,
    Unavailable,
    Oversized,
    ReadBudget,
    NonUtf8,
    IdentityChanged,
}

fn retain_file(
    store: &Store,
    run_id: &str,
    root: &Path,
    path: &Path,
    expected: Option<&str>,
    limits: Limits,
    reads: &mut Reads,
) -> Result<std::result::Result<crate::model::runs::Artifact, Skip>> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(Err(Skip::Unavailable));
    };
    if !metadata.file_type().is_file() {
        return Ok(Err(Skip::Nonregular));
    }
    let Ok(canonical) = path.canonicalize() else {
        return Ok(Err(Skip::Unavailable));
    };
    if !canonical.starts_with(root) {
        return Ok(Err(Skip::OutsideRoot));
    }
    let Ok(file) = fs::File::open(&canonical) else {
        return Ok(Err(Skip::Unavailable));
    };
    let Ok(metadata) = file.metadata() else {
        return Ok(Err(Skip::Unavailable));
    };
    if !metadata.is_file() {
        return Ok(Err(Skip::Nonregular));
    }
    let remaining = limits.total_read_bytes.saturating_sub(reads.bytes);
    if remaining == 0 {
        return Ok(Err(Skip::ReadBudget));
    }
    if metadata.len() > limits.per_file_bytes {
        return Ok(Err(Skip::Oversized));
    }
    if metadata.len() > remaining {
        return Ok(Err(Skip::ReadBudget));
    }
    let maximum = limits.per_file_bytes.min(remaining);
    let (content, result) = read_bounded(file, maximum);
    reads.bytes += content.len() as u64;
    if result.is_err() {
        return Ok(Err(Skip::Unavailable));
    }
    if reads.bytes > limits.total_read_bytes {
        return Ok(Err(Skip::ReadBudget));
    }
    if content.len() as u64 > maximum {
        return Ok(Err(Skip::Oversized));
    }
    if std::str::from_utf8(&content).is_err() {
        return Ok(Err(Skip::NonUtf8));
    }
    let digest = format!("{:x}", Sha256::digest(&content));
    if expected != Some(digest.as_str()) {
        return Ok(Err(Skip::IdentityChanged));
    }
    // Publish exactly the bytes checked above: reopening the pathname could
    // copy changed/unbounded content under a different identity.
    let name = canonical.file_name().unwrap().to_string_lossy();
    Ok(Ok(store.add_artifact_bytes(
        run_id,
        &name,
        "source-snapshot",
        &content,
    )?))
}

fn read_bounded(file: fs::File, maximum: u64) -> (Vec<u8>, io::Result<usize>) {
    let mut bytes = Vec::new();
    let result = file.take(maximum.saturating_add(1)).read_to_end(&mut bytes);
    (bytes, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Store, Run) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("store")).unwrap();
        let run:Run=serde_json::from_value(json!({"id":"source-retention","name":"source","status":"running","command":[],"cwd":root.path().canonicalize().unwrap(),"runtime":"none","started_at_us":0,"diagnostics":[],"source":{"fingerprint":"original-source-identity","source_files":{}},"correctness":{}})).unwrap();
        store.save_run(&run).unwrap();
        (root, store, run)
    }
    fn add(run: &mut Run, name: &str, bytes: &[u8]) {
        fs::write(Path::new(&run.cwd).join(name), bytes).unwrap();
        run.source["source_files"][name] = json!(format!("{:x}", Sha256::digest(bytes)));
    }
    #[test]
    fn source_file_grown_after_metadata_is_read_only_through_the_sentinel() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("grown.py");
        fs::write(&path, b"small").unwrap();
        let file = fs::File::open(&path).unwrap();
        assert!(file.metadata().unwrap().len() <= 8);
        fs::write(&path, vec![b'x'; 1024]).unwrap();
        let (bytes, result) = read_bounded(file, 8);
        result.unwrap();
        assert_eq!(bytes.len(), 9);
    }
    #[test]
    fn exact_source_limits_preserve_original_artifact_and_fingerprint() {
        let (_root, store, mut run) = fixture();
        add(&mut run, "one.py", b"1234");
        add(&mut run, "two.py", b"5678");
        retain_with_limits(
            &store,
            &mut run,
            Limits {
                per_file_bytes: 4,
                total_read_bytes: 8,
            },
        )
        .unwrap();
        assert_eq!(run.source["source_retention"]["bytes"], 8);
        assert_eq!(run.source["source_retention"]["skipped_files"], 0);
        assert_eq!(run.source["fingerprint"], "original-source-identity");
        assert_eq!(run.source["retained_sources"].as_array().unwrap().len(), 2);
        let mut contents = store
            .artifacts(&run.id)
            .unwrap()
            .into_iter()
            .map(|a| fs::read(store.artifact_path(&run.id, &a.id).unwrap()).unwrap())
            .collect::<Vec<_>>();
        contents.sort();
        assert_eq!(contents, vec![b"1234".to_vec(), b"5678".to_vec()]);
    }
    #[test]
    fn mismatched_reads_are_charged_to_the_shared_total_budget() {
        let (_root, store, mut run) = fixture();
        for name in ["a.py", "b.py", "c.py"] {
            add(&mut run, name, b"data");
        }
        run.source["source_files"]["a.py"] = json!("previous-identity");
        retain_with_limits(
            &store,
            &mut run,
            Limits {
                per_file_bytes: 4,
                total_read_bytes: 8,
            },
        )
        .unwrap();
        assert_eq!(
            run.source["source_retention"]["read_budget"]["work"]["bytes"],
            8
        );
        assert_eq!(run.source["source_retention"]["bytes"], 4);
        assert_eq!(run.source["source_retention"]["skipped_files"], 2);
        assert_eq!(
            run.source["source_retention"]["read_budget"]["work"]["skips"]["identity_changed"],
            1
        );
        assert_eq!(
            run.source["source_retention"]["read_budget"]["work"]["skips"]["read_budget"],
            1
        );
    }
    #[test]
    fn grown_and_static_nonregular_paths_are_skipped_without_reading_or_partial_publication() {
        let (_root, store, mut run) = fixture();
        add(&mut run, "grown.py", b"old");
        fs::write(Path::new(&run.cwd).join("grown.py"), vec![b'x'; 1024]).unwrap();
        fs::create_dir(Path::new(&run.cwd).join("directory.py")).unwrap();
        run.source["source_files"]["directory.py"] = json!("unused");
        retain_with_limits(
            &store,
            &mut run,
            Limits {
                per_file_bytes: 8,
                total_read_bytes: 16,
            },
        )
        .unwrap();
        assert_eq!(
            run.source["source_retention"]["read_budget"]["work"]["bytes"],
            0
        );
        assert_eq!(run.source["source_retention"]["skipped_files"], 2);
        assert!(store.artifacts(&run.id).unwrap().is_empty());
    }
}

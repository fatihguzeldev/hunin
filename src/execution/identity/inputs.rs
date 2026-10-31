//! Explicit workload input files snapshotted before launch.
use super::file_hash;
use crate::{model::runs::Run, storage::Store};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(in crate::execution) fn workload_inputs(
    cwd: &Path,
    paths: &[PathBuf],
) -> anyhow::Result<Value> {
    let mut files = BTreeMap::new();
    for path in paths {
        let actual = cwd
            .join(path)
            .canonicalize()
            .with_context(|| format!("declared input does not exist: {}", path.display()))?;
        let metadata = fs::metadata(&actual)?;
        if !metadata.is_file() {
            anyhow::bail!("declared input must be a regular file: {}", path.display());
        }
        let key = actual
            .strip_prefix(cwd)
            .unwrap_or(&actual)
            .to_string_lossy()
            .to_string();
        files.insert(
            key.clone(),
            json!({"path":key,"sha256":file_hash(&actual)?,"bytes":metadata.len()}),
        );
    }
    let rows = files.into_values().collect::<Vec<_>>();
    Ok(
        json!({"fingerprint":format!("{:x}",Sha256::digest(serde_json::to_vec(&rows)?)),"files":rows,"coverage":"explicit files snapshotted before launch only; stdin is closed, dynamic or network inputs are not inferred"}),
    )
}

/// Custody of explicitly declared inputs; this is not proof the target read them.
pub(in crate::execution) fn retain_workload_inputs(store: &Store, run: &mut Run) -> Result<()> {
    const PER_FILE: u64 = 128 * 1024 * 1024;
    const TOTAL: u64 = 256 * 1024 * 1024;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut read = 0u64;
    let mut unavailable = 0usize;
    let mut changed = false;
    let Some(files) = run.source["workload_inputs"]["files"].as_array_mut() else {
        return Ok(());
    };
    for (index, file) in files.iter_mut().enumerate() {
        let result = (|| -> Result<_> {
            ensure!(index < 128, "declared input custody file limit exceeded");
            let path = file["path"]
                .as_str()
                .context("declared input path missing")?;
            let source = Path::new(&run.cwd).join(path);
            let maximum = PER_FILE.min(TOTAL.saturating_sub(read));
            let artifact = store.add_artifact_controlled(
                &run.id,
                &source,
                "workload-input",
                maximum,
                &mut read,
                || {
                    ensure!(
                        Instant::now() < deadline,
                        "declared input custody checkpoint elapsed"
                    );
                    Ok(())
                },
            )?;
            Ok(artifact)
        })();
        match result {
            Ok(artifact) => {
                file["artifact_id"] = json!(artifact.id);
                if file["sha256"].as_str() != Some(artifact.sha256.as_str())
                    || file["bytes"].as_u64() != Some(artifact.size)
                {
                    file["snapshot_status"] = json!("changed");
                    file["retained_sha256"] = json!(artifact.sha256);
                    changed = true;
                    break;
                }
                file["snapshot_status"] = json!("retained");
            }
            Err(error) => {
                unavailable += 1;
                file["snapshot_status"] = json!("unavailable");
                file["snapshot_diagnostic"] = json!(format!("{error:#}"));
            }
        }
    }
    if changed {
        store.save_run(run)?;
        anyhow::bail!(
            "declared workload input changed between admission and prelaunch custody; target was not started"
        );
    }
    run.source["workload_inputs"]["custody"] = json!({"policy":"declared-input-custody-v1","scope":"prelaunch snapshot of explicitly declared files; target consumption and later file stability are not established","maximum_file_bytes":PER_FILE,"maximum_total_bytes":TOTAL,"maximum_files":128,"checkpoint_budget_ms":15_000,"bytes_read":read,"unavailable_files":unavailable});
    if unavailable > 0 {
        run.diagnostics.push(format!("{unavailable} declared workload input snapshots are unavailable; see workload_inputs.files; their original content digests remain metadata"));
    }
    store.save_run(run)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(root: &Path, store: &Store) -> Run {
        fs::write(root.join("guest.bin"), b"original").unwrap();
        let mut run:Run=serde_json::from_value(json!({"id":"inputs","name":"inputs","status":"running","command":["not-started"],"cwd":root,"runtime":"none","started_at_us":1,"finished_at_us":null,"exit_code":null,"diagnostics":[],"source":{},"correctness":{}})).unwrap();
        run.source["workload_inputs"] =
            workload_inputs(root, &[PathBuf::from("guest.bin")]).unwrap();
        store.save_run(&run).unwrap();
        run
    }

    #[test]
    fn changed_declared_input_cannot_be_presented_as_the_original_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        let mut run = declared(root.path(), &store);
        fs::write(root.path().join("guest.bin"), b"changed").unwrap();
        assert!(
            retain_workload_inputs(&store, &mut run)
                .unwrap_err()
                .to_string()
                .contains("changed")
        );
        let saved = store.run(&run.id).unwrap();
        let input = &saved.source["workload_inputs"]["files"][0];
        assert_eq!(input["snapshot_status"], "changed");
        assert_ne!(input["retained_sha256"], input["sha256"]);
        let retained = store
            .artifact_path(&run.id, input["artifact_id"].as_str().unwrap())
            .unwrap();
        assert_eq!(fs::read(retained).unwrap(), b"changed");
    }

    #[test]
    fn unavailable_declared_bytes_remain_digest_metadata_only() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        let mut run = declared(root.path(), &store);
        fs::remove_file(root.path().join("guest.bin")).unwrap();
        retain_workload_inputs(&store, &mut run).unwrap();
        let input = &run.source["workload_inputs"]["files"][0];
        assert_eq!(input["snapshot_status"], "unavailable");
        assert!(input["artifact_id"].is_null());
        assert!(input["sha256"].is_string());
        assert!(store.artifacts(&run.id).unwrap().is_empty());
    }
}

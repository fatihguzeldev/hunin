use crate::{model::runs::Run, storage::Store};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

/// Retain bounded source text before launching the measured process. A matching
/// snapshot identifies recorded bytes, independently of a binary/source mapping.
pub(crate) fn retain(store: &Store, run: &mut Run) -> Result<()> {
    let root = Path::new(&run.cwd);
    let mut snapshots = Vec::new();
    let mut skipped = 0;
    let mut bytes = 0;
    for (relative, expected) in run.source["source_files"].as_object().into_iter().flatten() {
        let path = root.join(relative);
        let Ok(canonical) = path.canonicalize() else {
            skipped += 1;
            continue;
        };
        if !canonical.starts_with(root) {
            skipped += 1;
            continue;
        }
        let Ok(content) = fs::read(&canonical) else {
            skipped += 1;
            continue;
        };
        if content.len() > 2 * 1024 * 1024
            || bytes + content.len() > 32 * 1024 * 1024
            || std::str::from_utf8(&content).is_err()
        {
            skipped += 1;
            continue;
        }
        let digest = format!("{:x}", Sha256::digest(&content));
        if expected.as_str() != Some(digest.as_str()) {
            skipped += 1;
            continue;
        }
        let artifact = store.add_artifact(&run.id, &canonical, "source-snapshot")?;
        if artifact.sha256 != digest {
            skipped += 1;
            continue;
        }
        bytes += content.len();
        snapshots.push(json!({"path":relative,"artifact_id":artifact.id,"sha256":digest}));
    }
    run.source["retained_sources"] = json!(snapshots);
    run.source["source_retention"] = json!({"bytes":bytes,"skipped_files":skipped,"scope":"pre-launch UTF-8 project files, at most 2 MiB per file and 32 MiB per recording","build_mapping_verified":false});
    Ok(())
}

pub(crate) fn read(store: &Store, params: &Value) -> Result<Value> {
    let run = store.run(params["run_id"].as_str().unwrap())?;
    let root = PathBuf::from(&run.cwd);
    let root = root.canonicalize().unwrap_or_else(|_| normalize(&root));
    let requested = params["path"].as_str().unwrap();
    let requested = if requested.starts_with("file:") {
        url::Url::parse(requested)?
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("source file URL cannot be represented on this host"))?
    } else {
        PathBuf::from(requested)
    };
    let joined = root.join(requested);
    let path = joined.canonicalize().unwrap_or_else(|_| normalize(&joined));
    let relative = path
        .strip_prefix(&root)
        .context("source path is outside the measured project")?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    let retained = run.source["retained_sources"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| {
            entry["path"]
                .as_str()
                .is_some_and(|p| p.replace('\\', "/") == relative)
        });
    let (text, identity_verified, diagnostic) = if let Some(entry) = retained {
        let artifact = store.artifact_path(
            &run.id,
            entry["artifact_id"]
                .as_str()
                .context("snapshot artifact identity missing")?,
        )?;
        let content = fs::read(artifact)?;
        anyhow::ensure!(
            entry["sha256"].as_str() == Some(format!("{:x}", Sha256::digest(&content)).as_str()),
            "retained source integrity check failed"
        );
        (
            String::from_utf8(content)?,
            true,
            "Showing retained pre-launch source bytes. The frame-to-built-code mapping is separate and has not been verified.",
        )
    } else {
        let path = path.canonicalize()?;
        anyhow::ensure!(
            path.starts_with(root.canonicalize()?),
            "source path is outside the measured project"
        );
        if fs::metadata(&path)?.len() > 2 * 1024 * 1024 {
            bail!("source file exceeds 2 MiB view limit");
        }
        (
            fs::read_to_string(&path)?,
            false,
            "Showing current project source. Measured-build/source provenance has not been established.",
        )
    };
    let line = params["line"].as_u64().unwrap_or(1) as usize;
    let count = params["count"].as_u64().unwrap_or(80).min(500) as usize;
    Ok(
        json!({"path":path,"start_line":line,"content":text.lines().skip(line.saturating_sub(1)).take(count).collect::<Vec<_>>().join("\n"),"identity_verified":identity_verified,"source_kind":if identity_verified{"recorded_snapshot"}else{"current_file"},"build_mapping_verified":false,"diagnostic":diagnostic}),
    )
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

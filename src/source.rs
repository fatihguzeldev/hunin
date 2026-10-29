use crate::storage::Store;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

mod retention;
pub(crate) use retention::retain;

pub(crate) fn read(store: &Store, params: &Value) -> Result<Value> {
    let run = store.run(params["run_id"].as_str().unwrap())?;
    let requested_uri = params["path"].as_str().unwrap();
    let mut mapped = crate::recordings::source_snapshots(store, &run.id)?
        .into_iter()
        .filter(|s| source_key(s["uri"].as_str().unwrap_or("")) == source_key(requested_uri))
        .collect::<Vec<_>>();
    if !mapped.is_empty() {
        if let Some(code) = params["code_id"].as_str() {
            mapped.retain(|s| s["provenance"]["code_id"] == code || s["sha256"] == code);
            anyhow::ensure!(
                !mapped.is_empty(),
                "no retained source matches the requested code identity; current source is not substituted"
            );
        }
        let identities = mapped
            .iter()
            .filter_map(|s| s["sha256"].as_str())
            .collect::<std::collections::HashSet<_>>();
        anyhow::ensure!(
            identities.len() == 1,
            "multiple measured source versions exist for this URI; supply code_id"
        );
        let snapshot = &mapped[0];
        let content = fs::read(
            store.artifact_path(
                &run.id,
                snapshot["artifact_id"]
                    .as_str()
                    .context("retained source artifact missing")?,
            )?,
        )?;
        anyhow::ensure!(
            snapshot["sha256"].as_str() == Some(format!("{:x}", Sha256::digest(&content)).as_str()),
            "retained source integrity check failed"
        );
        let text = String::from_utf8(content)?;
        let window = source_window(&text, params);
        return Ok(
            json!({"path":requested_uri,"start_line":window.start_line,"content":window.content,"next_line":window.next_line,"total_lines":window.total_lines,"identity_verified":true,"source_kind":"recorded_source_map","build_mapping_verified":false,"provenance":snapshot["provenance"],"diagnostic":"Showing immutable source retained with the measured script and its runtime-linked mapping. Mapping provenance is retained; compiler semantic correctness is not independently proven."}),
        );
    }
    let root = PathBuf::from(&run.cwd);
    let root = retained_path(&root);
    let requested = params["path"].as_str().unwrap();
    let requested = if requested.starts_with("file:") {
        url::Url::parse(requested)?
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("source file URL cannot be represented on this host"))?
    } else {
        PathBuf::from(requested)
    };
    let joined = root.join(requested);
    let path = retained_path(&joined);
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
    let window = source_window(&text, params);
    Ok(
        json!({"path":path,"start_line":window.start_line,"content":window.content,"next_line":window.next_line,"total_lines":window.total_lines,"identity_verified":identity_verified,"source_kind":if identity_verified{"recorded_snapshot"}else{"current_file"},"build_mapping_verified":false,"diagnostic":diagnostic}),
    )
}

struct SourceWindow {
    start_line: usize,
    content: String,
    next_line: Option<usize>,
    total_lines: usize,
}

fn source_window(text: &str, params: &Value) -> SourceWindow {
    let start_line = params["line"].as_u64().unwrap_or(1) as usize;
    let count = params["count"].as_u64().unwrap_or(80).min(500) as usize;
    let start = start_line.saturating_sub(1);
    let mut total_lines = 0;
    let mut lines = Vec::new();
    for (index, line) in text.lines().enumerate() {
        total_lines = index + 1;
        if index >= start && lines.len() < count {
            lines.push(line);
        }
    }
    let next = start.saturating_add(lines.len());
    SourceWindow {
        start_line,
        content: lines.join("\n"),
        next_line: (next < total_lines).then_some(next.saturating_add(1)),
        total_lines,
    }
}

fn source_key(uri: &str) -> String {
    let path = if uri.starts_with("file:") {
        url::Url::parse(uri)
            .ok()
            .and_then(|u| u.to_file_path().ok())
    } else if !uri.contains("://") {
        Some(PathBuf::from(uri))
    } else {
        None
    };
    path.map(|p| {
        (if p.is_absolute() {
            retained_path(&p)
        } else {
            normalize(&p)
        })
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .to_owned()
    })
    .unwrap_or_else(|| uri.to_owned())
}

// A retained file can be gone while its parent still resolves through an OS or
// user symlink (for example /tmp -> /private/tmp). Resolve that existing prefix
// before comparing project boundaries or immutable source identities.
fn retained_path(path: &Path) -> PathBuf {
    for ancestor in path.ancestors() {
        if let Ok(canonical) = ancestor.canonicalize() {
            return normalize(&canonical.join(path.strip_prefix(ancestor).unwrap()));
        }
    }
    normalize(path)
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            #[cfg(windows)]
            Component::Prefix(prefix) => {
                use std::path::Prefix;
                match prefix.kind() {
                    Prefix::VerbatimDisk(drive) | Prefix::Disk(drive) => {
                        result.push(format!("{}:", drive.to_ascii_uppercase() as char));
                    }
                    Prefix::VerbatimUNC(server, share) => {
                        result.push(format!(
                            "\\\\{}\\{}",
                            server.to_string_lossy(),
                            share.to_string_lossy()
                        ));
                    }
                    _ => result.push(prefix.as_os_str()),
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

#[cfg(test)]
mod window_tests {
    use super::source_window;
    use serde_json::json;

    #[test]
    fn source_windows_preserve_line_semantics_and_distinguish_empty_content_from_end() {
        let first = source_window("first\r\n\r\nlast\r\n", &json!({"count":2}));
        assert_eq!(first.content, "first\n");
        assert_eq!(first.next_line, Some(3));
        assert_eq!(first.total_lines, 3);
        let last = source_window("first\n\nlast\n", &json!({"line":3,"count":2}));
        assert_eq!(last.content, "last");
        assert_eq!(last.next_line, None);
        assert_eq!(source_window("", &json!({})).total_lines, 0);
        assert_eq!(source_window("\n", &json!({})).total_lines, 1);
        assert_eq!(source_window("one", &json!({"line":5001})).next_line, None);
    }

    #[test]
    fn source_window_count_remains_bounded_and_existing_default_is_preserved() {
        let text = (0..1000).map(|_| "line").collect::<Vec<_>>().join("\n");
        assert_eq!(source_window(&text, &json!({})).next_line, Some(81));
        let window = source_window(&text, &json!({"count":5000}));
        assert_eq!(window.content.lines().count(), 500);
        assert_eq!(window.next_line, Some(501));
    }
}

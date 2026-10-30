//! Bounded project snapshot and the separately observed launched executable.
use super::file_hash;
use crate::host::resolve_launch_executable;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn git(cwd: &Path, args: &[&str]) -> Option<Vec<u8>> {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| o.stdout)
}

pub(in crate::execution) fn source(
    cwd: &Path,
    command: &[String],
    data_root: Option<&Path>,
) -> Value {
    let revision =
        git(cwd, &["rev-parse", "HEAD"]).map(|v| String::from_utf8_lossy(&v).trim().to_owned());
    let status = git(cwd, &["status", "--porcelain"]);
    let mut hash = Sha256::new();
    let mut complete = true;
    let mut files = std::collections::BTreeMap::new();
    if let Some(ref v) = revision {
        hash.update(v.as_bytes());
    }
    if let Some(diff) = git(cwd, &["diff", "--binary", "HEAD"]) {
        hash.update(diff);
    } else if revision.is_some() {
        complete = false;
    }
    if let Some(untracked) = git(cwd, &["ls-files", "--others", "--exclude-standard", "-z"]) {
        let entries = untracked
            .split(|c| *c == 0)
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>();
        if entries.len() > 5000 {
            complete = false;
        }
        for path in entries.into_iter().take(5000) {
            let p = PathBuf::from(String::from_utf8_lossy(path).as_ref());
            hash.update(path);
            match file_hash(&cwd.join(p)) {
                Ok(value) => hash.update(value.as_bytes()),
                Err(_) => complete = false,
            }
        }
    }
    let launch_executable = command
        .first()
        .and_then(|s| resolve_launch_executable(s, cwd, &BTreeMap::new()));
    let executable = command.first().and_then(|s| {
        resolve_launch_executable(s, cwd, &BTreeMap::new())
            .and_then(|path| path.canonicalize().ok())
    });
    let executable_hash = executable.as_ref().and_then(|p| file_hash(p).ok());
    let mut snapshot_complete = true;
    snapshot(cwd, cwd, data_root, &mut files, &mut snapshot_complete);
    if revision.is_none() {
        complete = snapshot_complete;
        if files.is_empty() {
            complete = false;
        }
        hash = Sha256::new();
        hash.update(serde_json::to_vec(&files).unwrap());
    }
    json!({"revision":revision,"dirty":status.as_ref().map(|s|!s.is_empty()),"fingerprint":if complete{format!("{:x}",hash.finalize())}else{"unknown".into()},"source_snapshot_complete":snapshot_complete,"source_snapshot_scope":"bounded project source/config files; dependency/build/cache and Hunin data directories excluded","source_files":files,"launch_executable":launch_executable,"executable":executable,"executable_sha256":executable_hash,"build_source_verified":false,"diagnostic":"Source snapshot identity and launched executable identity are recorded separately; their build relationship and dynamically loaded code are not assumed. Incomplete/absent source snapshots remain unknown."})
}

fn snapshot(
    root: &Path,
    directory: &Path,
    data_root: Option<&Path>,
    files: &mut std::collections::BTreeMap<String, String>,
    complete: &mut bool,
) {
    if files.len() >= 5000 {
        *complete = false;
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        *complete = false;
        return;
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if data_root.is_some_and(|d| {
            path.starts_with(d.join("captures")) || path.starts_with(d.join("artifacts"))
        }) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            *complete = false;
            continue;
        };
        if kind.is_dir() {
            if name.starts_with('.')
                || [
                    "target",
                    "node_modules",
                    "__pycache__",
                    "venv",
                    "dist",
                    "build",
                ]
                .contains(&name.as_ref())
            {
                continue;
            }
            snapshot(root, &path, data_root, files, complete);
        } else if kind.is_symlink() {
            *complete = false;
        } else if kind.is_file()
            && path.extension().and_then(|s| s.to_str()).is_some_and(|e| {
                [
                    "rs", "c", "h", "cc", "cpp", "cxx", "hpp", "go", "py", "pyi", "js", "mjs",
                    "cjs", "jsx", "ts", "tsx", "mts", "cts", "json", "toml", "yaml", "yml", "lock",
                ]
                .contains(&e)
            })
        {
            if files.len() >= 5000
                || entry
                    .metadata()
                    .map(|m| m.len() > 8 * 1024 * 1024)
                    .unwrap_or(true)
            {
                *complete = false;
                continue;
            }
            match file_hash(&path) {
                Ok(hash) => {
                    files.insert(
                        path.strip_prefix(root)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .into_owned(),
                        hash,
                    );
                }
                Err(_) => *complete = false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn module_typescript_sources_participate_in_gitless_snapshot_identity() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("input.json"), "{\"work\":42}").unwrap();
        fs::write(root.path().join("workload.mts"), "export const work = 1;\n").unwrap();
        fs::write(
            root.path().join("support.cts"),
            "export const support = 1;\n",
        )
        .unwrap();
        let first = source(root.path(), &[], None);
        assert!(first["source_files"]["workload.mts"].is_string());
        assert!(first["source_files"]["support.cts"].is_string());
        assert_eq!(first["source_snapshot_complete"], true);
        assert_eq!(first["build_source_verified"], false);
        fs::write(root.path().join("workload.mts"), "export const work = 2;\n").unwrap();
        let changed_module = source(root.path(), &[], None);
        assert_ne!(first["fingerprint"], changed_module["fingerprint"]);
        fs::write(
            root.path().join("support.cts"),
            "export const support = 2;\n",
        )
        .unwrap();
        assert_ne!(
            changed_module["fingerprint"],
            source(root.path(), &[], None)["fingerprint"]
        );
    }

    #[test]
    fn non_git_source_changes_have_distinct_snapshot_identity() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("app.py");
        fs::write(&path, "print(1)\n").unwrap();
        let a = source(root.path(), &["python3".into(), "app.py".into()], None);
        fs::write(&path, "print(2)\n").unwrap();
        let b = source(root.path(), &["python3".into(), "app.py".into()], None);
        assert_ne!(a["fingerprint"], b["fingerprint"]);
        assert_eq!(b["source_snapshot_complete"], true);
    }
    #[test]
    fn absent_source_does_not_turn_directory_identity_into_source_evidence() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(source(root.path(), &[], None)["fingerprint"], "unknown");
    }
}

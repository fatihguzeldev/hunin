use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
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

pub fn source(cwd: &Path, command: &[String], data_root: Option<&Path>) -> Value {
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
    let executable = command.first().and_then(|s| resolve(s, cwd));
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
    json!({"revision":revision,"dirty":status.as_ref().map(|s|!s.is_empty()),"fingerprint":if complete{format!("{:x}",hash.finalize())}else{"unknown".into()},"source_snapshot_complete":snapshot_complete,"source_snapshot_scope":"bounded project source/config files; dependency/build/cache and Hunin data directories excluded","source_files":files,"executable":executable,"executable_sha256":executable_hash,"build_source_verified":false,"diagnostic":"Source snapshot identity and launched executable identity are recorded separately; their build relationship and dynamically loaded code are not assumed. Incomplete/absent source snapshots remain unknown."})
}

fn file_hash(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
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
                    "cjs", "jsx", "ts", "tsx", "json", "toml", "yaml", "yml", "lock",
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

pub fn resolve(name: &str, cwd: &Path) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.is_absolute() || p.components().count() > 1 {
        return cwd.join(p).canonicalize().ok();
    }
    let names = if cfg!(windows) && p.extension().is_none() {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .map(|ext| format!("{name}{ext}"))
            .collect::<Vec<_>>()
    } else {
        vec![name.to_owned()]
    };
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|p| p.is_file())
        .and_then(|p| p.canonicalize().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
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

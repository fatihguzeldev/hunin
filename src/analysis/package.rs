use crate::storage::Store;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

const MAX_FILES: usize = 2000;
const MAX_BYTES: u64 = 64 * 1024 * 1024;

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn executable(command: &str, cwd: &Path) -> Result<PathBuf> {
    let path = PathBuf::from(command);
    if path.is_absolute() || path.components().count() > 1 {
        return cwd
            .join(path)
            .canonicalize()
            .context("analyzer executable does not exist");
    }
    let mut names = vec![command.to_owned()];
    if cfg!(windows) && path.extension().is_none() {
        names.extend(
            std::env::var("PATHEXT")
                .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
                .split(';')
                .map(|suffix| format!("{command}{suffix}")),
        );
    }
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        for name in &names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return candidate
                    .canonicalize()
                    .context("resolve analyzer executable");
            }
        }
    }
    bail!("analyzer prerequisite missing: executable {command:?} was not found in PATH")
}
fn relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        bail!("package files must be relative paths without parent traversal");
    }
    Ok(())
}
fn collect(
    source: &Path,
    relative: &Path,
    files: &mut BTreeMap<String, Value>,
    destination: &Path,
    total: &mut u64,
    explicit: bool,
) -> Result<()> {
    let full = source.join(relative);
    if !full.canonicalize()?.starts_with(source) {
        bail!(
            "analyzer file escapes its package through a symlink: {}",
            relative.display()
        );
    }
    let metadata = fs::symlink_metadata(&full)
        .with_context(|| format!("read package file {}", full.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "analyzer package symlinks are not reproducible: {}",
            relative.display()
        );
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(full)? {
            let entry = entry?;
            let name = entry.file_name();
            if !explicit
                && matches!(
                    name.to_str(),
                    Some(
                        ".git"
                            | "node_modules"
                            | "target"
                            | "__pycache__"
                            | ".venv"
                            | "venv"
                            | ".env"
                            | ".DS_Store"
                    )
                )
            {
                continue;
            }
            collect(
                source,
                &relative.join(name),
                files,
                destination,
                total,
                explicit,
            )?;
        }
    } else if metadata.is_file() {
        let key = relative.to_string_lossy().replace('\\', "/");
        if files.contains_key(&key) {
            return Ok(());
        }
        if files.len() >= MAX_FILES {
            bail!("analyzer package exceeds {MAX_FILES} files; declare only the required files");
        }
        *total += metadata.len();
        if *total > MAX_BYTES {
            bail!("analyzer package exceeds 64 MiB; declare only the required files");
        }
        let bytes = fs::read(&full)?;
        let target = destination.join(relative);
        fs::create_dir_all(target.parent().context("package file has no parent")?)?;
        fs::write(&target, &bytes)?;
        fs::set_permissions(target, metadata.permissions())?;
        files.insert(key, json!({"sha256":digest(&bytes),"size":bytes.len()}));
    } else {
        bail!(
            "analyzer package includes a special file: {}",
            relative.display()
        );
    }
    Ok(())
}
pub fn register(store: &Store, params: &Value) -> Result<Value> {
    let name = params["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .context("analyzer name is required")?;
    let command = params["command"]
        .as_array()
        .filter(|items| !items.is_empty())
        .context("analyzer command is required")?;
    let mut command: Vec<String> = command
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .context("command arguments must be nonempty strings")
        })
        .collect::<Result<_>>()?;
    let cwd = PathBuf::from(
        params["cwd"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or(std::env::current_dir()?.display().to_string()),
    )
    .canonicalize()?;
    if !cwd.is_dir() {
        bail!("analyzer cwd must be a directory");
    }
    let required = params
        .get("requirements")
        .filter(|v| v.is_object())
        .context("declared applicability requirements are required")?;
    super::applicability::validate_declaration(required)?;
    let original_executable = executable(&command[0], &cwd)?;
    let executable_sha256 = digest(&fs::read(&original_executable)?);
    let executable_is_packaged = original_executable.starts_with(&cwd);
    for (index, argument) in command.iter_mut().enumerate() {
        let path = PathBuf::from(&*argument);
        if path.is_absolute() && path.starts_with(&cwd) {
            *argument = path.strip_prefix(&cwd)?.to_string_lossy().into_owned();
            if index == 0 {
                *argument = format!("./{argument}");
            }
        } else if index > 0 && path.is_absolute() && path.exists() {
            bail!("analyzer argument names an unmanaged file outside its package: {argument}");
        }
    }
    let base = store.root().join("analysis-packages");
    fs::create_dir_all(&base)?;
    let staging = base.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    let source = staging.join("source");
    fs::create_dir_all(&source)?;
    let result = (|| -> Result<Value> {
        let mut files = BTreeMap::new();
        let mut total = 0;
        if let Some(list) = params["files"].as_array() {
            if list.is_empty() {
                bail!("analyzer files cannot be empty");
            }
            for file in list {
                let path = PathBuf::from(file.as_str().context("package files must be strings")?);
                relative(&path)?;
                collect(&cwd, &path, &mut files, &source, &mut total, true)?;
            }
        } else {
            collect(&cwd, Path::new(""), &mut files, &source, &mut total, false)?;
        }
        if files.is_empty() {
            bail!("analyzer package contains no files");
        }
        let dependencies = params
            .get("dependencies")
            .cloned()
            .unwrap_or_else(|| json!({}));
        super::dependencies::validate(&dependencies, &source)?;
        let identity = json!({"command":command,"files":files,"requirements":required,"version":params["version"].as_str().unwrap_or("1"),"dependencies":dependencies,"executable_sha256":executable_sha256});
        let package_digest = digest(&serde_json::to_vec(&identity)?);
        let id = package_digest.clone();
        let managed = base.join(&id);
        let mut analysis = json!({"id":id,"name":name,"version":params["version"].as_str().unwrap_or("1"),"command":command,"requirements":required,"dependencies":dependencies,"package_digest":package_digest,"files":files,"package_dir":managed,"created_at_us":crate::model::now_us(),"executable":{"path":original_executable,"sha256":executable_sha256,"packaged":executable_is_packaged},"dependency_state":"declared"});
        if !executable_is_packaged {
            analysis["command"][0] = json!(original_executable);
        }
        fs::write(
            staging.join("manifest.json"),
            serde_json::to_vec_pretty(&analysis)?,
        )?;
        if managed.exists() {
            fs::remove_dir_all(&staging)?;
        } else {
            fs::rename(&staging, &managed)?;
        }
        store.save_document("analyses", &id, &analysis)?;
        Ok(json!({"analysis":analysis}))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}
pub fn verify(analysis: &Value) -> Result<PathBuf> {
    let source = PathBuf::from(
        analysis["package_dir"]
            .as_str()
            .context("managed package path missing")?,
    )
    .join("source");
    for (path, identity) in analysis["files"]
        .as_object()
        .context("package file manifest missing")?
    {
        let target = source.join(path);
        if !target.canonicalize()?.starts_with(&source)
            || fs::symlink_metadata(&target)?.file_type().is_symlink()
            || digest(&fs::read(&target)?) != identity["sha256"]
        {
            bail!(
                "managed analyzer package changed; register its new version before reuse: {path}"
            );
        }
    }
    let program = analysis["executable"]["path"]
        .as_str()
        .context("analyzer executable identity missing")?;
    let program = if analysis["executable"]["packaged"] == true {
        executable(analysis["command"][0].as_str().unwrap(), &source)?
    } else {
        PathBuf::from(program)
    };
    if digest(&fs::read(program).context("analyzer executable prerequisite unavailable")?)
        != analysis["executable"]["sha256"]
    {
        bail!("analyzer executable version changed; register against the new runtime before reuse");
    }
    Ok(source)
}

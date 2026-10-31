//! Bounded package discovery, staging and immutable registration.
use super::{
    MAX_BYTES, MAX_FILES,
    identity::{executable, relative},
    source_read_error,
};
use crate::{
    analysis::{applicability, dependencies, files},
    storage::Store,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

struct InventoryLimits {
    entries: usize,
    depth: usize,
    path_bytes: usize,
}
impl Default for InventoryLimits {
    fn default() -> Self {
        Self {
            entries: 8000,
            depth: 64,
            path_bytes: 8 * 1024 * 1024,
        }
    }
}
#[derive(Default)]
struct InventoryUsage {
    entries: usize,
    path_bytes: usize,
}
impl InventoryUsage {
    fn entry(&mut self, limits: &InventoryLimits) -> Result<()> {
        self.entries = self.entries.saturating_add(1);
        anyhow::ensure!(
            self.entries <= limits.entries,
            "analyzer package inventory exceeds {} entries; declare only required paths",
            limits.entries
        );
        Ok(())
    }
    fn path(&mut self, path: &Path, limits: &InventoryLimits) -> Result<()> {
        let depth = path
            .components()
            .filter(|part| matches!(part, Component::Normal(_)))
            .count();
        anyhow::ensure!(
            depth <= limits.depth,
            "analyzer package inventory exceeds {} path components",
            limits.depth
        );
        self.path_bytes = self.path_bytes.saturating_add(path.to_string_lossy().len());
        anyhow::ensure!(
            self.path_bytes <= limits.path_bytes,
            "analyzer package inventory exceeds {} relative-path bytes",
            limits.path_bytes
        );
        Ok(())
    }
}

struct InventoryControl<'a> {
    usage: InventoryUsage,
    limits: &'a InventoryLimits,
    checkpoint: &'a mut dyn FnMut() -> Result<()>,
}
impl InventoryControl<'_> {
    fn check(&mut self) -> Result<()> {
        (self.checkpoint)()
    }
    fn entry(&mut self) -> Result<()> {
        self.usage.entry(self.limits)
    }
    fn path(&mut self, path: &Path) -> Result<()> {
        self.usage.path(path, self.limits)
    }
}

fn collect(
    source: &Path,
    relative: &Path,
    files: &mut BTreeMap<String, Value>,
    destination: &Path,
    total: &mut u64,
    explicit: bool,
    control: &mut InventoryControl<'_>,
) -> Result<()> {
    control.check()?;
    control.path(relative)?;
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
            control.check()?;
            let entry = entry?;
            control.entry()?;
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
                control,
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
        let bytes = files::read(&full, MAX_BYTES - *total, || control.check())
            .map_err(source_read_error)?;
        *total += bytes.len() as u64;
        let target = destination.join(relative);
        fs::create_dir_all(target.parent().context("package file has no parent")?)?;
        fs::write(&target, &bytes)?;
        fs::set_permissions(target, metadata.permissions())?;
        files.insert(
            key,
            json!({"sha256":files::digest(&bytes),"size":bytes.len()}),
        );
    } else {
        bail!(
            "analyzer package includes a special file: {}",
            relative.display()
        );
    }
    control.check()
}

pub(in crate::analysis) fn register(store: &Store, params: &Value) -> Result<Value> {
    register_controlled(store, params, &InventoryLimits::default(), || Ok(()))
}
fn register_controlled(
    store: &Store,
    params: &Value,
    limits: &InventoryLimits,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<Value> {
    checkpoint()?;
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
    applicability::validate_declaration(required)?;
    crate::analysis::output::validate_contract(params.get("output_contract"))?;
    let original_executable = executable(&command[0], &cwd)?;
    let executable_sha256 = files::hash(&original_executable, &mut checkpoint)?;
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
        let mut control = InventoryControl {
            usage: InventoryUsage::default(),
            limits,
            checkpoint: &mut checkpoint,
        };
        if let Some(list) = params["files"].as_array() {
            if list.is_empty() {
                bail!("analyzer files cannot be empty");
            }
            for file in list {
                control.check()?;
                control.entry()?;
                let path = PathBuf::from(file.as_str().context("package files must be strings")?);
                relative(&path)?;
                collect(
                    &cwd,
                    &path,
                    &mut files,
                    &source,
                    &mut total,
                    true,
                    &mut control,
                )?;
            }
        } else {
            control.entry()?;
            collect(
                &cwd,
                Path::new(""),
                &mut files,
                &source,
                &mut total,
                false,
                &mut control,
            )?;
        }
        if files.is_empty() {
            bail!("analyzer package contains no files");
        }
        let dependencies = params
            .get("dependencies")
            .cloned()
            .unwrap_or_else(|| json!({}));
        dependencies::validate(&dependencies, &source)?;
        let mut identity = json!({"command":command,"files":files,"requirements":required,"version":params["version"].as_str().unwrap_or("1"),"dependencies":dependencies,"executable_sha256":executable_sha256});
        if let Some(contract) = params.get("output_contract") {
            identity["output_contract"] = contract.clone();
        }
        let package_digest = files::digest(&serde_json::to_vec(&identity)?);
        let id = package_digest.clone();
        let managed = base.join(&id);
        let mut analysis = json!({"id":id,"name":name,"version":params["version"].as_str().unwrap_or("1"),"command":command,"requirements":required,"dependencies":dependencies,"package_digest":package_digest,"files":files,"package_dir":managed,"created_at_us":crate::model::now_us(),"executable":{"path":original_executable,"sha256":executable_sha256,"packaged":executable_is_packaged},"dependency_state":"declared"});
        if let Some(contract) = params.get("output_contract") {
            analysis["output_contract"] = contract.clone();
        }
        if !executable_is_packaged {
            analysis["command"][0] = json!(original_executable);
        }
        fs::write(
            staging.join("manifest.json"),
            serde_json::to_vec_pretty(&analysis)?,
        )?;
        checkpoint()?;
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

#[cfg(test)]
mod tests;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Resolve a launch path without following its final symlink.
pub(crate) fn resolve_launch_executable(
    name: &str,
    cwd: &Path,
    overrides: &BTreeMap<String, String>,
) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.is_absolute() || p.components().count() > 1 {
        let path = cwd.join(p);
        return path.is_file().then_some(path);
    }
    let names = if cfg!(windows) && p.extension().is_none() {
        overrides
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("PATHEXT"))
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var("PATHEXT").ok())
            .unwrap_or_else(|| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .map(|ext| format!("{name}{ext}"))
            .collect::<Vec<_>>()
    } else {
        vec![name.to_owned()]
    };
    let path = overrides
        .iter()
        .find(|(k, _)| {
            if cfg!(windows) {
                k.eq_ignore_ascii_case("PATH")
            } else {
                k.as_str() == "PATH"
            }
        })
        .map(|(_, v)| std::ffi::OsString::from(v))
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    std::env::split_paths(&path)
        .map(|directory| {
            if directory.is_absolute() {
                directory
            } else {
                cwd.join(directory)
            }
        })
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn launch_resolution_rejects_directories_and_keeps_relative_path_search() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("programs")).unwrap();
        let binary = if cfg!(windows) { "work.EXE" } else { "work" };
        let launch = root.path().join("programs").join(binary);
        fs::write(&launch, "controlled executable bytes").unwrap();
        let overrides = BTreeMap::from([
            ("PATH".into(), "programs".into()),
            ("PATHEXT".into(), ".EXE;.COM".into()),
        ]);
        assert_eq!(
            resolve_launch_executable("work", root.path(), &overrides),
            Some(launch)
        );
        assert!(resolve_launch_executable("./programs", root.path(), &overrides).is_none());
        assert!(
            resolve_launch_executable(root.path().to_str().unwrap(), root.path(), &overrides)
                .is_none()
        );
    }
}

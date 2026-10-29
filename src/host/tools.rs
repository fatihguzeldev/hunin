use std::{env, path::PathBuf};

/// Find a host tool through its explicit override, then the inherited search path.
pub(crate) fn tool(name: &str) -> Option<PathBuf> {
    if let Some(path) = env::var_os(format!("HUNIN_{}", name.to_uppercase())) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    find_on_path(name)
}

pub(crate) fn find_on_path(name: &str) -> Option<PathBuf> {
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let path = directory.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.into()
        });
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

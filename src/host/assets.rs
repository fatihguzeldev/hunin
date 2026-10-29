use std::{
    env,
    path::{Path, PathBuf},
};

pub(crate) fn assets() -> PathBuf {
    env::var_os("HUNIN_ASSETS_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .filter(|p| p.join("collectors").is_dir())
        })
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

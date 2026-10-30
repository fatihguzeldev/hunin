//! Source URI and retained-path identity across missing files and host aliases.
use std::path::{Component, Path, PathBuf};

pub(super) fn source_key(uri: &str) -> String {
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
pub(super) fn retained_path(path: &Path) -> PathBuf {
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

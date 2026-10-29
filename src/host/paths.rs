use std::path::Path;

/// Use ordinary drive/UNC spelling at external process boundaries.
/// Stored identities and other device namespaces remain unchanged.
pub(crate) fn tool_path(path: &Path) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        use std::{
            ffi::OsString,
            os::windows::ffi::{OsStrExt, OsStringExt},
            path::{Component, Prefix},
        };
        let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if let Some(Component::Prefix(prefix)) = path.components().next() {
            match prefix.kind() {
                Prefix::VerbatimDisk(_) => return OsString::from_wide(&units[4..]).into(),
                Prefix::VerbatimUNC(_, _) => {
                    let mut ordinary = vec![b'\\' as u16, b'\\' as u16];
                    ordinary.extend_from_slice(&units[8..]);
                    return OsString::from_wide(&ordinary).into();
                }
                _ => {}
            }
        }
    }
    path.to_path_buf()
}

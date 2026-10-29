//! Filesystem object snapshots used by authorized cleanup, not an OS lease.
use anyhow::Result;
#[cfg(windows)]
use anyhow::ensure;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    volume: String,
    file: String,
    created: Option<String>,
    pub(super) kind: PathKind,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum PathKind {
    Directory,
    File,
    Link,
    Other,
}

pub(super) fn path_kind(metadata: &fs::Metadata) -> PathKind {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return PathKind::Link;
        }
    }
    if metadata.is_symlink() {
        PathKind::Link
    } else if metadata.is_dir() {
        PathKind::Directory
    } else if metadata.is_file() {
        PathKind::File
    } else {
        PathKind::Other
    }
}

pub(super) fn identity(path: &Path) -> Result<Option<Identity>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let created = metadata
        .created()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| format!("{}:{}", value.as_secs(), value.subsec_nanos()));
    #[cfg(unix)]
    let (volume, file) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev().to_string(), metadata.ino().to_string())
    };
    #[cfg(windows)]
    let (volume, file) = {
        use std::{
            mem::MaybeUninit,
            os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
        };
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            GetFileInformationByHandle,
        };
        let handle = fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        ensure!(
            unsafe { GetFileInformationByHandle(handle.as_raw_handle().cast(), info.as_mut_ptr()) }
                != 0,
            "cannot confirm managed file identity: {}",
            std::io::Error::last_os_error()
        );
        let info = unsafe { info.assume_init() };
        (
            info.dwVolumeSerialNumber.to_string(),
            format!("{}:{}", info.nFileIndexHigh, info.nFileIndexLow),
        )
    };
    #[cfg(not(any(unix, windows)))]
    anyhow::bail!("managed path identity is unavailable on this host");
    #[cfg(any(unix, windows))]
    Ok(Some(Identity {
        volume,
        file,
        created,
        kind: path_kind(&metadata),
    }))
}

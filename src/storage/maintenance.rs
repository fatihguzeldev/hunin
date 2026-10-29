use super::Store;
use crate::model::runs::{Run, RunStatus};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::Path};

const NAMESPACE: &str = "run-removals";
const CANDIDATE_LIMIT: usize = 4096;
const PATH_LIMIT: usize = 1024;
const RECEIPT_BYTE_LIMIT: usize = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    run_id: String,
    authorized_at_us: u64,
    completed: bool,
    artifact_directory: Identity,
    artifacts: Vec<Blob>,
    paths: Vec<ManagedPath>,
    notes: Vec<String>,
    last_error: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Blob {
    sha256: String,
    disposition: Disposition,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Disposition {
    Pending,
    Removed,
    Missing,
    Referenced,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedPath {
    area: Area,
    id: String,
    parent: Identity,
    identity: Identity,
    disposition: Disposition,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Area {
    Capture,
    AnalyzerExecution,
}
impl Area {
    fn directory(&self) -> &'static str {
        match self {
            Self::Capture => "captures",
            Self::AnalyzerExecution => "analysis-executions",
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Identity {
    volume: String,
    file: String,
    created: Option<String>,
    kind: PathKind,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PathKind {
    Directory,
    File,
    Link,
    Other,
}

impl Store {
    /// The existing explicit removal operation is also its retry entry point.
    /// A receipt authorizes only the original run identity and observed paths.
    pub fn remove_run(&self, run_id: &str) -> Result<()> {
        self.authorize_removal(run_id)?;
        self.finish_removal(run_id).with_context(|| {
            format!("recording metadata removed; cleanup incomplete; inspect removal receipt {run_id} and retry the same removal")
        })
    }

    /// Exact facts of an already-authorized removal; no new deletion authority.
    pub fn removal_receipt(&self, run_id: &str) -> Result<Option<Value>> {
        receipt_text(&self.connection()?, run_id)?
            .map(|text| {
                let value: Value = serde_json::from_str(&text)?;
                ensure!(value.is_object(), "removal receipt is not an object");
                Ok::<_, anyhow::Error>(value)
            })
            .transpose()
    }

    fn authorize_removal(&self, run_id: &str) -> Result<()> {
        ensure!(
            run_id.len() <= 4096,
            "recording ID exceeds removal receipt bound"
        );
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if load_receipt(&tx, run_id)?.is_some() {
            ensure_absent(&tx, run_id)?;
            tx.commit()?;
            return Ok(());
        }
        let text: String = tx
            .query_row("SELECT manifest FROM runs WHERE id=?1", [run_id], |r| {
                r.get(0)
            })
            .context("recording not found and no authorized removal receipt exists")?;
        let run: Run = serde_json::from_str(&text)?;
        ensure!(
            run.status != RunStatus::Running,
            "cancel a running recording before removing it"
        );
        ensure!(tx.query_row("SELECT COUNT(*) FROM documents WHERE owner_run_id=?1 AND json_extract(payload,'$.status')='running'", [run_id], |r| r.get::<_,u64>(0))? == 0,
            "cancel active analyzer executions before removing this recording");
        let hashes = tx
            .prepare(
                "SELECT DISTINCT sha256 FROM artifacts WHERE run_id=?1 ORDER BY sha256 LIMIT ?2",
            )?
            .query_map(params![run_id, CANDIDATE_LIMIT + 1], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ensure!(
            hashes.len() <= CANDIDATE_LIMIT,
            "removal exceeds the {CANDIDATE_LIMIT} distinct artifact candidate bound"
        );
        ensure!(
            hashes.iter().all(|sha| is_artifact_digest(sha)),
            "invalid artifact digest; removal did not change recording metadata"
        );
        let attempts = tx.prepare("SELECT id FROM documents WHERE namespace='analysis-results' AND owner_run_id=?1 ORDER BY id LIMIT ?2")?
            .query_map(params![run_id,PATH_LIMIT], |r|r.get::<_,String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ensure!(
            attempts.len() < PATH_LIMIT,
            "removal exceeds its managed path bound"
        );
        let artifact_directory =
            identity(&self.root.join("artifacts"))?.context("artifact directory is missing")?;
        ensure!(
            artifact_directory.kind == PathKind::Directory,
            "artifact directory is not an owned directory"
        );
        let mut receipt = Receipt {
            version: 1,
            run_id: run_id.into(),
            authorized_at_us: crate::model::now_us(),
            completed: false,
            artifact_directory,
            artifacts: hashes
                .into_iter()
                .map(|sha256| Blob {
                    sha256,
                    disposition: Disposition::Pending,
                })
                .collect(),
            paths: vec![],
            notes: vec![],
            last_error: None,
        };
        self.capture_removal_path(&mut receipt, Area::Capture, run_id)?;
        for id in attempts {
            self.capture_removal_path(&mut receipt, Area::AnalyzerExecution, &id)?;
        }
        // Receipt ownership deliberately does not cascade with the removed run.
        save_receipt(&tx, &receipt)?;
        tx.execute("DELETE FROM runs WHERE id=?1", [run_id])?;
        tx.commit()?;
        Ok(())
    }

    fn capture_removal_path(&self, receipt: &mut Receipt, area: Area, id: &str) -> Result<()> {
        if !managed_id(id) {
            receipt.notes.push(format!(
                "{} path was not eligible: ID is not one bounded filesystem component",
                area.directory()
            ));
            return Ok(());
        }
        let parent = self.root.join(area.directory());
        let Some(parent_identity) = identity(&parent)? else {
            return Ok(());
        };
        ensure!(
            parent_identity.kind == PathKind::Directory,
            "managed directory parent is linked or not a directory; no removal authorized"
        );
        if let Some(path_identity) = identity(&parent.join(id))? {
            receipt.paths.push(ManagedPath {
                area,
                id: id.into(),
                parent: parent_identity,
                identity: path_identity,
                disposition: Disposition::Pending,
            });
        }
        Ok(())
    }

    fn finish_removal(&self, run_id: &str) -> Result<()> {
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut receipt =
            load_receipt(&tx, run_id)?.context("authorized removal receipt is missing")?;
        let outcome = self.clean_receipt(&tx, &mut receipt);
        receipt.last_error = outcome
            .as_ref()
            .err()
            .map(|error| bounded_error(&format!("{error:#}")));
        if outcome.is_ok() {
            receipt.completed = true;
        }
        // Successful filesystem steps remain retryable if interruption occurs
        // before this progress commit: absent paths are idempotent completion.
        save_receipt(&tx, &receipt)?;
        tx.commit()?;
        outcome
    }

    fn clean_receipt(&self, tx: &Connection, receipt: &mut Receipt) -> Result<()> {
        ensure_absent(tx, &receipt.run_id)?;
        ensure!(
            receipt.version == 1
                && receipt.artifacts.len() <= CANDIDATE_LIMIT
                && receipt.paths.len() <= PATH_LIMIT,
            "invalid removal receipt shape"
        );
        ensure!(
            receipt
                .artifacts
                .iter()
                .all(|blob| is_artifact_digest(&blob.sha256)),
            "invalid artifact digest in removal receipt"
        );
        if receipt.completed {
            return Ok(());
        }
        let root = self.root.join("artifacts");
        ensure!(
            identity(&root)?.as_ref() == Some(&receipt.artifact_directory),
            "artifact directory identity changed or is unavailable"
        );
        for blob in &mut receipt.artifacts {
            if blob.disposition != Disposition::Pending {
                continue;
            }
            let referenced: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM artifacts WHERE sha256=?1)",
                [&blob.sha256],
                |r| r.get(0),
            )?;
            blob.disposition = if referenced {
                Disposition::Referenced
            } else {
                let path = root.join(&blob.sha256);
                match fs::symlink_metadata(&path) {
                    Ok(metadata) => {
                        ensure!(
                            path_kind(&metadata) == PathKind::File,
                            "unreferenced blob is not a regular file; preserved {}",
                            blob.sha256
                        );
                        fs::remove_file(path)?;
                        Disposition::Removed
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        Disposition::Missing
                    }
                    Err(error) => return Err(error.into()),
                }
            };
        }
        for path in &mut receipt.paths {
            if path.disposition != Disposition::Pending {
                continue;
            }
            ensure!(
                managed_id(&path.id),
                "invalid managed path in removal receipt"
            );
            if matches!(path.area, Area::AnalyzerExecution) {
                let reused:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM documents WHERE namespace='analysis-results' AND id=?1)",[&path.id],|r|r.get(0))?;
                ensure!(
                    !reused,
                    "analyzer execution identity was reused; its workspace was preserved"
                );
            }
            let parent = self.root.join(path.area.directory());
            ensure!(
                identity(&parent)?.as_ref() == Some(&path.parent),
                "managed directory parent identity changed or is unavailable"
            );
            let target = parent.join(&path.id);
            match identity(&target)? {
                None => path.disposition = Disposition::Missing,
                Some(current) => {
                    ensure!(
                        current == path.identity,
                        "managed path identity changed; preserved {}",
                        target.display()
                    );
                    if current.kind == PathKind::Directory {
                        fs::remove_dir_all(&target)?;
                    } else {
                        #[cfg(windows)]
                        if current.kind == PathKind::Link && fs::symlink_metadata(&target)?.is_dir()
                        {
                            fs::remove_dir(&target)?;
                            path.disposition = Disposition::Removed;
                            continue;
                        }
                        fs::remove_file(&target)?;
                    }
                    path.disposition = Disposition::Removed;
                }
            }
        }
        Ok(())
    }
}

fn ensure_absent(connection: &Connection, run_id: &str) -> Result<()> {
    let reused: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1)",
        [run_id],
        |r| r.get(0),
    )?;
    ensure!(
        !reused,
        "recording identity was reused; prior removal receipt does not authorize deleting the new recording"
    );
    Ok(())
}

fn load_receipt(connection: &Connection, run_id: &str) -> Result<Option<Receipt>> {
    receipt_text(connection, run_id)?
        .map(|text| {
            let receipt: Receipt = serde_json::from_str(&text)?;
            ensure!(receipt.version == 1, "unsupported removal receipt version");
            ensure!(
                receipt.run_id == run_id,
                "removal receipt recording identity mismatch"
            );
            Ok(receipt)
        })
        .transpose()
}

fn receipt_text(connection: &Connection, run_id: &str) -> Result<Option<String>> {
    // Refuse oversized retained payloads before copying their text into Rust.
    let row: Option<(u64, Option<String>)> = connection
        .query_row(
            "SELECT length(CAST(payload AS BLOB)),CASE WHEN length(CAST(payload AS BLOB))<=?3 THEN payload END FROM documents WHERE namespace=?1 AND id=?2",
            params![NAMESPACE, run_id, RECEIPT_BYTE_LIMIT],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(bytes, text)| {
        ensure!(
            bytes <= RECEIPT_BYTE_LIMIT as u64,
            "removal receipt exceeds its byte bound"
        );
        text.context("removal receipt payload is unavailable")
    })
    .transpose()
}

fn save_receipt(connection: &Connection, receipt: &Receipt) -> Result<()> {
    let payload = serde_json::to_string(receipt)?;
    ensure!(
        payload.len() <= RECEIPT_BYTE_LIMIT,
        "removal receipt exceeds its byte bound"
    );
    connection.execute("INSERT INTO documents(namespace,id,payload,updated_at_us,owner_run_id) VALUES(?1,?2,?3,?4,NULL) ON CONFLICT(namespace,id) DO UPDATE SET payload=excluded.payload,updated_at_us=excluded.updated_at_us,owner_run_id=NULL",params![NAMESPACE,receipt.run_id,payload,crate::model::now_us()])?;
    Ok(())
}

pub(super) fn is_artifact_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn managed_id(id: &str) -> bool {
    if id.len() > 4096 {
        return false;
    }
    let mut components = Path::new(id).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

fn bounded_error(error: &str) -> String {
    let mut end = error.len().min(2048);
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    error[..end].into()
}

fn path_kind(metadata: &fs::Metadata) -> PathKind {
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

fn identity(path: &Path) -> Result<Option<Identity>> {
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

#[cfg(test)]
mod tests;

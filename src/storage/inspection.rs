//! Metadata-only inspection. Observations never authorize deletion.
use super::Store;
use anyhow::{Result, ensure};
use rusqlite::{Connection, OpenFlags, params};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct InspectionLimits {
    pub max_artifact_rows: usize,
    pub max_filesystem_entries: usize,
    pub max_findings: usize,
    /// A checkpoint budget, not preemption of one filesystem/SQLite operation
    /// or SQLite's initial WAL-index recovery.
    pub time_budget: Duration,
}

impl Default for InspectionLimits {
    fn default() -> Self {
        Self {
            max_artifact_rows: 1000,
            max_filesystem_entries: 1000,
            max_findings: 500,
            time_budget: Duration::from_secs(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectionArea {
    ArtifactReferences,
    ArtifactFiles,
    Captures,
    AnalyzerExecutions,
    AnalyzerPackages,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectionScanStatus {
    NotScanned,
    Exhausted,
    Limited,
    Unavailable,
}

#[derive(Clone, Debug)]
pub struct InspectionScan {
    pub area: InspectionArea,
    pub examined: usize,
    pub status: InspectionScanStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectionLimit {
    ArtifactRows,
    FilesystemEntries,
    Findings,
    Time,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectionPathKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceMetadataReference {
    Present,
    Absent,
    Unknown,
}

#[derive(Clone, Debug)]
pub enum InspectionFinding {
    MissingReferencedBlob {
        sha256: String,
        example_run_id: String,
        example_artifact_id: String,
    },
    /// No SQL reference in the read snapshot; content and ownership are unverified.
    UnreferencedAtSnapshot {
        sha256: String,
        size_bytes: u64,
    },
    PendingOwnershipUnknown {
        relative_path: PathBuf,
        kind: InspectionPathKind,
        size_bytes: Option<u64>,
    },
    WorkspaceOwnershipUnknown {
        relative_path: PathBuf,
        kind: InspectionPathKind,
        metadata_reference: WorkspaceMetadataReference,
    },
    UnfollowedPath {
        relative_path: PathBuf,
        kind: InspectionPathKind,
    },
    PathUnavailable {
        relative_path: PathBuf,
        reason: String,
    },
    InvalidArtifactReference {
        row_ordinal: usize,
        reason: String,
    },
}

#[derive(Clone, Debug)]
pub struct StorageInspection {
    pub findings: Vec<InspectionFinding>,
    pub scans: Vec<InspectionScan>,
    pub limits_hit: Vec<InspectionLimit>,
    pub elapsed: Duration,
    pub database_changed_during_scan: bool,
    pub content_hashes_checked: usize,
}

impl Store {
    /// Read one SQL reference snapshot and bounded live filesystem metadata.
    /// No bootstrap, migration, blob reads, ownership inference or repair occurs.
    /// The filesystem is not an atomic snapshot of those references. A live
    /// publisher can expose bytes before committing their reference, so even an
    /// exhausted scan and an unchanged database never authorize deletion.
    pub fn inspect(&self, limits: InspectionLimits) -> Result<StorageInspection> {
        ensure!(
            limits.max_artifact_rows > 0
                && limits.max_filesystem_entries > 0
                && limits.max_findings > 0
                && !limits.time_budget.is_zero(),
            "inspection budgets must be positive"
        );
        let started = Instant::now();
        // Deliberately do not call Store::connection: inspection must not set
        // journal mode, create a database or run bootstrap/migrations.
        let mut connection = Connection::open_with_flags(
            self.root.join("hunin.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.busy_timeout(Duration::ZERO)?;
        let before: u64 = connection.query_row("PRAGMA data_version", [], |row| row.get(0))?;
        let transaction = connection.transaction()?;
        let schema: u32 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        ensure!(
            schema <= super::SCHEMA_VERSION,
            "inspection cannot interpret newer schema {schema}"
        );
        let areas = [
            InspectionArea::ArtifactReferences,
            InspectionArea::ArtifactFiles,
            InspectionArea::Captures,
            InspectionArea::AnalyzerExecutions,
            InspectionArea::AnalyzerPackages,
        ];
        let mut inspector = Inspector {
            root: &self.root,
            connection: &transaction,
            limits,
            started,
            filesystem_entries: 0,
            finding_paths: HashSet::new(),
            report: StorageInspection {
                findings: Vec::new(),
                scans: areas
                    .iter()
                    .map(|&area| InspectionScan {
                        area,
                        examined: 0,
                        status: InspectionScanStatus::NotScanned,
                    })
                    .collect(),
                limits_hit: Vec::new(),
                elapsed: Duration::ZERO,
                database_changed_during_scan: false,
                content_hashes_checked: 0,
            },
        };
        inspector.references()?;
        for (area, directory) in [
            (InspectionArea::ArtifactFiles, "artifacts"),
            (InspectionArea::Captures, "captures"),
            (InspectionArea::AnalyzerExecutions, "analysis-executions"),
            (InspectionArea::AnalyzerPackages, "analysis-packages"),
        ] {
            inspector.directory(area, directory)?;
        }
        let mut report = inspector.report;
        transaction.commit()?;
        let after: u64 = connection.query_row("PRAGMA data_version", [], |row| row.get(0))?;
        report.database_changed_during_scan = before != after;
        report.elapsed = started.elapsed();
        Ok(report)
    }
}

struct Inspector<'a> {
    root: &'a Path,
    connection: &'a Connection,
    limits: InspectionLimits,
    started: Instant,
    filesystem_entries: usize,
    finding_paths: HashSet<PathBuf>,
    report: StorageInspection,
}

impl Inspector<'_> {
    fn scan(&mut self, area: InspectionArea) -> &mut InspectionScan {
        self.report
            .scans
            .iter_mut()
            .find(|scan| scan.area == area)
            .unwrap()
    }

    fn limited(&mut self, area: InspectionArea, limit: InspectionLimit) {
        self.scan(area).status = InspectionScanStatus::Limited;
        if !self.report.limits_hit.contains(&limit) {
            self.report.limits_hit.push(limit);
        }
    }

    fn checkpoint(&mut self, area: InspectionArea) -> bool {
        if self.started.elapsed() >= self.limits.time_budget {
            self.limited(area, InspectionLimit::Time);
            return false;
        }
        if self.report.findings.len() >= self.limits.max_findings {
            self.limited(area, InspectionLimit::Findings);
            return false;
        }
        true
    }

    fn finding(&mut self, finding: InspectionFinding) {
        let path = match &finding {
            InspectionFinding::MissingReferencedBlob { sha256, .. }
            | InspectionFinding::UnreferencedAtSnapshot { sha256, .. } => {
                Some(Path::new("artifacts").join(sha256))
            }
            InspectionFinding::PendingOwnershipUnknown { relative_path, .. }
            | InspectionFinding::WorkspaceOwnershipUnknown { relative_path, .. }
            | InspectionFinding::UnfollowedPath { relative_path, .. }
            | InspectionFinding::PathUnavailable { relative_path, .. } => {
                Some(relative_path.clone())
            }
            InspectionFinding::InvalidArtifactReference { .. } => None,
        };
        if path.is_some_and(|path| !self.finding_paths.insert(path)) {
            return;
        }
        if self.report.findings.len() < self.limits.max_findings {
            self.report.findings.push(finding);
        }
    }

    fn unavailable(&mut self, area: InspectionArea, path: &Path, error: std::io::Error) {
        self.finding(InspectionFinding::PathUnavailable {
            relative_path: path.to_owned(),
            reason: error.to_string(),
        });
        self.scan(area).status = InspectionScanStatus::Unavailable;
    }

    fn root_directory(&mut self, area: InspectionArea, relative: &str) -> Option<bool> {
        match fs::symlink_metadata(self.root.join(relative)) {
            Ok(metadata) if metadata.is_dir() && !metadata.is_symlink() => Some(true),
            Ok(metadata) => {
                self.finding(InspectionFinding::UnfollowedPath {
                    relative_path: relative.into(),
                    kind: kind(&metadata),
                });
                self.scan(area).status = InspectionScanStatus::Unavailable;
                None
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(false),
            Err(error) => {
                self.unavailable(area, Path::new(relative), error);
                None
            }
        }
    }

    fn references(&mut self) -> Result<()> {
        let area = InspectionArea::ArtifactReferences;
        if !self.checkpoint(area) || self.root_directory(area, "artifacts").is_none() {
            return Ok(());
        }
        // Read only keys, never the artifact/document JSON or blob contents.
        let connection = self.connection;
        let mut statement = connection
            .prepare("SELECT run_id,id,sha256 FROM artifacts ORDER BY run_id,id LIMIT ?1")?;
        let mut rows = statement.query([self.limits.max_artifact_rows])?;
        let mut seen = HashSet::new();
        loop {
            if !self.checkpoint(area) {
                return Ok(());
            }
            if self.scan(area).examined >= self.limits.max_artifact_rows {
                self.limited(area, InspectionLimit::ArtifactRows);
                return Ok(());
            }
            let Some(row) = rows.next()? else {
                if self.scan(area).status != InspectionScanStatus::Unavailable {
                    self.scan(area).status = InspectionScanStatus::Exhausted;
                }
                return Ok(());
            };
            self.scan(area).examined += 1;
            let row_ordinal = self.scan(area).examined;
            let run_value = row.get_ref(0)?;
            let id_value = row.get_ref(1)?;
            let sha_value = row.get_ref(2)?;
            let keys = (run_value.as_str(), id_value.as_str(), sha_value.as_str());
            let (Ok(run_id), Ok(artifact_id), Ok(sha256)) = keys else {
                self.finding(InspectionFinding::InvalidArtifactReference {
                    row_ordinal,
                    reason: "artifact reference keys are not valid text".into(),
                });
                continue;
            };
            if run_id.len() > 4096 || artifact_id.len() > 4096 || !is_digest(sha256) {
                self.finding(InspectionFinding::InvalidArtifactReference {
                    row_ordinal,
                    reason: "artifact key exceeds its inspection bound or digest is not canonical"
                        .into(),
                });
                continue;
            }
            if !seen.insert(sha256.to_owned()) {
                continue;
            }
            let relative = Path::new("artifacts").join(sha256);
            match fs::symlink_metadata(self.root.join(&relative)) {
                Ok(metadata) if metadata.is_file() && !metadata.is_symlink() => {}
                Ok(metadata) => self.finding(InspectionFinding::UnfollowedPath {
                    relative_path: relative,
                    kind: kind(&metadata),
                }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.finding(InspectionFinding::MissingReferencedBlob {
                        sha256: sha256.into(),
                        example_run_id: run_id.into(),
                        example_artifact_id: artifact_id.into(),
                    });
                }
                Err(error) => self.unavailable(area, &relative, error),
            }
        }
    }

    fn directory(&mut self, area: InspectionArea, directory: &str) -> Result<()> {
        if !self.checkpoint(area) {
            return Ok(());
        }
        match self.root_directory(area, directory) {
            Some(true) => {}
            Some(false) => {
                self.scan(area).status = InspectionScanStatus::Exhausted;
                return Ok(());
            }
            None => return Ok(()),
        }
        let mut entries = match fs::read_dir(self.root.join(directory)) {
            Ok(entries) => entries,
            Err(error) => {
                self.unavailable(area, Path::new(directory), error);
                return Ok(());
            }
        };
        loop {
            if !self.checkpoint(area) {
                return Ok(());
            }
            if self.filesystem_entries >= self.limits.max_filesystem_entries {
                self.limited(area, InspectionLimit::FilesystemEntries);
                return Ok(());
            }
            let Some(entry) = entries.next() else {
                if self.scan(area).status != InspectionScanStatus::Unavailable {
                    self.scan(area).status = InspectionScanStatus::Exhausted;
                }
                return Ok(());
            };
            self.filesystem_entries += 1;
            self.scan(area).examined += 1;
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.unavailable(area, Path::new(directory), error);
                    return Ok(());
                }
            };
            let name = entry.file_name();
            let relative = Path::new(directory).join(&name);
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(error) => {
                    self.unavailable(area, &relative, error);
                    continue;
                }
            };
            if area == InspectionArea::ArtifactFiles {
                if name
                    .to_str()
                    .is_some_and(|name| name.starts_with(".pending-"))
                {
                    self.finding(InspectionFinding::PendingOwnershipUnknown {
                        relative_path: relative,
                        kind: kind(&metadata),
                        size_bytes: metadata.is_file().then_some(metadata.len()),
                    });
                } else if metadata.is_file()
                    && !metadata.is_symlink()
                    && name.to_str().is_some_and(is_digest)
                {
                    let sha256 = name.to_str().unwrap();
                    let referenced: bool = self.connection.query_row(
                        "SELECT EXISTS(SELECT 1 FROM artifacts WHERE sha256=?1)",
                        [sha256],
                        |row| row.get(0),
                    )?;
                    if !referenced {
                        self.finding(InspectionFinding::UnreferencedAtSnapshot {
                            sha256: sha256.into(),
                            size_bytes: metadata.len(),
                        });
                    }
                } else {
                    self.finding(InspectionFinding::UnfollowedPath {
                        relative_path: relative,
                        kind: kind(&metadata),
                    });
                }
            } else {
                let metadata_reference = match name.to_str() {
                    Some(id) => {
                        let exists: bool = match area {
                            InspectionArea::Captures => self.connection.query_row(
                                "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1)",
                                [id],
                                |row| row.get(0),
                            )?,
                            InspectionArea::AnalyzerExecutions
                            | InspectionArea::AnalyzerPackages => {
                                let namespace = if area == InspectionArea::AnalyzerExecutions {
                                    "analysis-results"
                                } else {
                                    "analyses"
                                };
                                self.connection.query_row(
                                    "SELECT EXISTS(SELECT 1 FROM documents WHERE namespace=?1 AND id=?2)",
                                    params![namespace,id], |row| row.get(0),
                                )?
                            }
                            _ => unreachable!(),
                        };
                        if exists {
                            WorkspaceMetadataReference::Present
                        } else {
                            WorkspaceMetadataReference::Absent
                        }
                    }
                    None => WorkspaceMetadataReference::Unknown,
                };
                self.finding(InspectionFinding::WorkspaceOwnershipUnknown {
                    relative_path: relative,
                    kind: kind(&metadata),
                    metadata_reference,
                });
            }
        }
    }
}

fn is_digest(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn kind(metadata: &fs::Metadata) -> InspectionPathKind {
    if metadata.is_symlink() {
        InspectionPathKind::Symlink
    } else if metadata.is_file() {
        InspectionPathKind::File
    } else if metadata.is_dir() {
        InspectionPathKind::Directory
    } else {
        InspectionPathKind::Other
    }
}

#[cfg(test)]
mod tests;

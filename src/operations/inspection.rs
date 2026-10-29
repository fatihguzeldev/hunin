use crate::storage::{
    InspectionArea, InspectionFinding, InspectionLimit, InspectionLimits, InspectionPathKind,
    InspectionScanStatus, StorageInspection, Store, WorkspaceMetadataReference,
};
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

/// Protocol projection owns display encoding. Paths are never repair tokens.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Finding {
    MissingReferencedBlob {
        sha256: String,
        example_run_id: String,
        example_artifact_id: String,
    },
    UnreferencedAtSnapshot {
        sha256: String,
        size_bytes: String,
    },
    PendingOwnershipUnknown {
        relative_path: String,
        path_kind: &'static str,
        size_bytes: Option<String>,
    },
    WorkspaceOwnershipUnknown {
        relative_path: String,
        path_kind: &'static str,
        metadata_reference: &'static str,
    },
    UnfollowedPath {
        relative_path: String,
        path_kind: &'static str,
    },
    PathUnavailable {
        relative_path: String,
        reason: String,
    },
    InvalidArtifactReference {
        row_ordinal: usize,
        reason: String,
    },
}

impl From<InspectionFinding> for Finding {
    fn from(value: InspectionFinding) -> Self {
        match value {
            InspectionFinding::MissingReferencedBlob {
                sha256,
                example_run_id,
                example_artifact_id,
            } => Self::MissingReferencedBlob {
                sha256,
                example_run_id,
                example_artifact_id,
            },
            InspectionFinding::UnreferencedAtSnapshot { sha256, size_bytes } => {
                Self::UnreferencedAtSnapshot {
                    sha256,
                    size_bytes: size_bytes.to_string(),
                }
            }
            InspectionFinding::PendingOwnershipUnknown {
                relative_path,
                kind,
                size_bytes,
            } => Self::PendingOwnershipUnknown {
                relative_path: relative_path.to_string_lossy().into_owned(),
                path_kind: path_kind(kind),
                size_bytes: size_bytes.map(|size| size.to_string()),
            },
            InspectionFinding::WorkspaceOwnershipUnknown {
                relative_path,
                kind,
                metadata_reference,
            } => Self::WorkspaceOwnershipUnknown {
                relative_path: relative_path.to_string_lossy().into_owned(),
                path_kind: path_kind(kind),
                metadata_reference: match metadata_reference {
                    WorkspaceMetadataReference::Present => "present",
                    WorkspaceMetadataReference::Absent => "absent",
                    WorkspaceMetadataReference::Unknown => "unknown",
                },
            },
            InspectionFinding::UnfollowedPath {
                relative_path,
                kind,
            } => Self::UnfollowedPath {
                relative_path: relative_path.to_string_lossy().into_owned(),
                path_kind: path_kind(kind),
            },
            InspectionFinding::PathUnavailable {
                relative_path,
                reason,
            } => Self::PathUnavailable {
                relative_path: relative_path.to_string_lossy().into_owned(),
                reason,
            },
            InspectionFinding::InvalidArtifactReference {
                row_ordinal,
                reason,
            } => Self::InvalidArtifactReference {
                row_ordinal,
                reason,
            },
        }
    }
}

fn path_kind(kind: InspectionPathKind) -> &'static str {
    match kind {
        InspectionPathKind::File => "file",
        InspectionPathKind::Directory => "directory",
        InspectionPathKind::Symlink => "symlink",
        InspectionPathKind::Other => "other",
    }
}

pub(super) fn inspect(store: &Store, params: &Value) -> Result<Value> {
    let limits = InspectionLimits {
        max_artifact_rows: params["artifact_row_limit"].as_u64().unwrap_or(1000) as usize,
        max_filesystem_entries: params["filesystem_entry_limit"].as_u64().unwrap_or(1000) as usize,
        max_findings: params["finding_limit"].as_u64().unwrap_or(500) as usize,
        time_budget: std::time::Duration::from_millis(params["budget_ms"].as_u64().unwrap_or(1000)),
    };
    let budgets = serde_json::json!({
        "artifact_row_limit":limits.max_artifact_rows,
        "filesystem_entry_limit":limits.max_filesystem_entries,
        "finding_limit":limits.max_findings,
        "budget_ms":limits.time_budget.as_millis(),
    });
    let StorageInspection {
        findings,
        scans,
        limits_hit,
        elapsed,
        database_changed_during_scan,
        content_hashes_checked,
    } = store.inspect(limits)?;
    let findings: Vec<Finding> = findings.into_iter().map(Into::into).collect();
    let scans: Vec<Value> = scans
        .into_iter()
        .map(|scan| {
            serde_json::json!({
                "area":match scan.area {
                    InspectionArea::ArtifactReferences => "artifact_references",
                    InspectionArea::ArtifactFiles => "artifact_files",
                    InspectionArea::Captures => "captures",
                    InspectionArea::AnalyzerExecutions => "analyzer_executions",
                    InspectionArea::AnalyzerPackages => "analyzer_packages",
                },
                "examined":scan.examined,
                "status":match scan.status {
                    InspectionScanStatus::NotScanned => "not_scanned",
                    InspectionScanStatus::Exhausted => "exhausted",
                    InspectionScanStatus::Limited => "limited",
                    InspectionScanStatus::Unavailable => "unavailable",
                },
            })
        })
        .collect();
    let limits_hit: Vec<&str> = limits_hit
        .into_iter()
        .map(|limit| match limit {
            InspectionLimit::ArtifactRows => "artifact_rows",
            InspectionLimit::FilesystemEntries => "filesystem_entries",
            InspectionLimit::Findings => "findings",
            InspectionLimit::Time => "time",
        })
        .collect();
    Ok(serde_json::json!({
        "findings":findings,
        "scans":scans,
        "limits_hit":limits_hit,
        "budgets":budgets,
        "elapsed_ms":elapsed.as_secs_f64()*1000.0,
        "database_changed_during_scan":database_changed_during_scan,
        "content_hashes_checked":content_hashes_checked,
        "deletion_authorized":false,
        "reference_view":"sqlite_read_transaction",
        "filesystem_view":"live_metadata",
        "path_encoding":"lossy_display_only",
        "limitations":[
            "An absent SQL reference is not proof of abandoned ownership. A publisher may have exposed bytes before committing its reference.",
            "Workspace metadata presence does not establish a live owner. Pending files and workspaces require separate ownership evidence before repair.",
            "No blob content is read or hashed, and symlinks are not intentionally followed. Path replacement races are not excluded.",
            "The time budget is checked between operations; an individual SQLite or filesystem operation is not preempted.",
            "A normal read-only SQLite connection may use WAL sidecars or recover its WAL index. This is inspection of an opened store, not a zero-write forensic image reader.",
            "Paths are lossy display strings and file sizes are exact decimal strings; neither is an action token.",
        ],
    }))
}

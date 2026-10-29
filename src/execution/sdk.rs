use crate::storage::Store;
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::{fs, io::BufRead, path::Path};

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RejectionStage {
    Decode,
    Acceptance,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum IngestionOutcome {
    Empty,
    Accepted {
        record_count: usize,
    },
    Rejected {
        stage: RejectionStage,
        diagnostic: String,
    },
}

impl IngestionOutcome {
    pub fn rejection(&self) -> Option<&str> {
        match self {
            Self::Rejected { diagnostic, .. } => Some(diagnostic),
            _ => None,
        }
    }
}

/// SDK decoding and acceptance have one explicit recording outcome. Decode
/// immutable retained bytes and never publish a valid prefix of a rejected spool.
pub(super) fn ingest(store: &Store, run_id: &str, directory: &Path) -> IngestionOutcome {
    let records = match read(store, run_id, directory) {
        Ok(records) => records,
        Err(error) => {
            return IngestionOutcome::Rejected {
                stage: RejectionStage::Decode,
                diagnostic: format!("SDK spool decoding failed: {error:#}"),
            };
        }
    };
    if records.is_empty() {
        return IngestionOutcome::Empty;
    }
    match store.accept(run_id, "sdk-spool-v1", &records) {
        Ok(_) => IngestionOutcome::Accepted {
            record_count: records.len(),
        },
        Err(error) => IngestionOutcome::Rejected {
            stage: RejectionStage::Acceptance,
            diagnostic: format!("SDK result set rejected atomically: {error:#}"),
        },
    }
}

fn read(store: &Store, run_id: &str, directory: &Path) -> Result<Vec<Value>> {
    let mut records = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if !path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            continue;
        }
        let artifact = store.add_artifact(run_id, &path, "hunin-jsonl")?;
        let retained = store.artifact_path(run_id, &artifact.id)?;
        for (index, line) in std::io::BufReader::new(fs::File::open(retained)?)
            .lines()
            .enumerate()
        {
            let line = line?;
            if !line.trim().is_empty() {
                records.push(serde_json::from_str(&line).with_context(|| {
                    format!("SDK artifact {} line {}", artifact.name, index + 1)
                })?);
            }
        }
    }
    Ok(records)
}

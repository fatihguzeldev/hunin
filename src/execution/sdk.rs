use crate::storage::Store;
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    io::{BufRead, Read, Seek, SeekFrom},
    path::Path,
};

const PRODUCER_DESCRIPTOR_LIMIT: usize = 64;
const PRODUCER_ID_BYTE_LIMIT: usize = 256;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RejectionStage {
    Decode,
    Acceptance,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum IngestionStatus {
    Empty,
    Accepted {
        record_count: usize,
    },
    Rejected {
        stage: RejectionStage,
        diagnostic: String,
    },
}

#[derive(Serialize)]
pub(super) struct IngestionOutcome {
    #[serde(flatten)]
    status: IngestionStatus,
    evidence: SpoolEvidence,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "snake_case")]
enum DeliveryKnowledge {
    #[default]
    Unknown,
}

#[derive(Default, Serialize)]
struct SpoolSnapshot {
    inventory_retained: bool,
    retained_artifact_count: usize,
    byte_termination_checked_artifact_count: usize,
    nonempty_artifact_count: usize,
    nonempty_without_final_lf_count: usize,
}

#[derive(Serialize)]
struct ProducerDeclaration {
    producer_id: String,
    /// The accepted JSON number's representation, without another numeric
    /// conversion through consumers such as JavaScript's Number.
    pid_json_number: String,
}

#[derive(Serialize)]
struct ProducerDeclarations {
    source: &'static str,
    entry_limit: usize,
    producer_id_byte_limit: usize,
    count: usize,
    truncated: bool,
    entries: Vec<ProducerDeclaration>,
}

#[derive(Serialize)]
struct SpoolEvidence {
    scope: &'static str,
    completion: DeliveryKnowledge,
    loss: DeliveryKnowledge,
    spool_snapshot: SpoolSnapshot,
    accepted_producer_declarations: ProducerDeclarations,
    limitations: [&'static str; 3],
}

impl Default for SpoolEvidence {
    fn default() -> Self {
        Self {
            scope: "retained_sdk_spool_snapshot",
            completion: DeliveryKnowledge::Unknown,
            loss: DeliveryKnowledge::Unknown,
            spool_snapshot: SpoolSnapshot::default(),
            accepted_producer_declarations: ProducerDeclarations {
                source: "accepted_record_fields",
                entry_limit: PRODUCER_DESCRIPTOR_LIMIT,
                producer_id_byte_limit: PRODUCER_ID_BYTE_LIMIT,
                count: 0,
                truncated: false,
                entries: Vec::new(),
            },
            limitations: [
                "Empty or accepted spools do not establish successful producer shutdown or delivery of all intended records; unflushed buffers and invisible producers can leave no evidence",
                "Producer IDs and PIDs are self-reported fields of accepted records, not observed OS identities or a complete producer inventory",
                "Final line-feed observations describe retained bytes only; they prove neither successful flush/fsync nor producer shutdown, and valid legacy final JSON without a line-feed remains accepted",
            ],
        }
    }
}

impl IngestionOutcome {
    pub fn rejection(&self) -> Option<&str> {
        match &self.status {
            IngestionStatus::Rejected { diagnostic, .. } => Some(diagnostic),
            _ => None,
        }
    }
}

/// SDK decoding and acceptance have one explicit recording outcome. Decode
/// immutable retained bytes and never publish a valid prefix of a rejected spool.
pub(super) fn ingest(store: &Store, run_id: &str, directory: &Path) -> IngestionOutcome {
    let mut evidence = SpoolEvidence::default();
    let records = match read(store, run_id, directory, &mut evidence.spool_snapshot) {
        Ok(records) => records,
        Err(error) => {
            return IngestionOutcome {
                status: IngestionStatus::Rejected {
                    stage: RejectionStage::Decode,
                    diagnostic: format!("SDK spool decoding failed: {error:#}"),
                },
                evidence,
            };
        }
    };
    if records.is_empty() {
        return IngestionOutcome {
            status: IngestionStatus::Empty,
            evidence,
        };
    }
    let status = match store.accept(run_id, "sdk-spool-v1", &records) {
        Ok(_) => {
            let producers = records
                .iter()
                .filter_map(|record| {
                    Some((
                        record["producer_id"].as_str()?,
                        record["pid"].as_number()?.to_string(),
                    ))
                })
                .collect::<BTreeSet<_>>();
            evidence.accepted_producer_declarations.count = producers.len();
            evidence.accepted_producer_declarations.entries = producers
                .into_iter()
                // Omit oversized declarations rather than truncate identity
                // strings into a different, potentially colliding producer ID.
                .filter(|(producer_id, _)| producer_id.len() <= PRODUCER_ID_BYTE_LIMIT)
                .take(PRODUCER_DESCRIPTOR_LIMIT)
                .map(|(producer_id, pid_json_number)| ProducerDeclaration {
                    producer_id: producer_id.into(),
                    pid_json_number,
                })
                .collect();
            evidence.accepted_producer_declarations.truncated =
                evidence.accepted_producer_declarations.entries.len()
                    < evidence.accepted_producer_declarations.count;
            IngestionStatus::Accepted {
                record_count: records.len(),
            }
        }
        Err(error) => IngestionStatus::Rejected {
            stage: RejectionStage::Acceptance,
            diagnostic: format!("SDK result set rejected atomically: {error:#}"),
        },
    };
    IngestionOutcome { status, evidence }
}

fn read(
    store: &Store,
    run_id: &str,
    directory: &Path,
    snapshot: &mut SpoolSnapshot,
) -> Result<Vec<Value>> {
    let mut records = Vec::new();
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if !path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            continue;
        }
        paths.push(path);
    }
    paths.sort();
    let mut retained = Vec::new();
    for path in paths {
        let artifact = store.add_artifact(run_id, &path, "hunin-jsonl")?;
        retained.push(artifact);
        snapshot.retained_artifact_count += 1;
    }
    // Snapshot every discovered spool before decoding any of them. A rejected
    // prefix must not hide later raw evidence or imply a complete inventory.
    snapshot.inventory_retained = true;
    for artifact in &retained {
        let mut file = fs::File::open(store.artifact_path(run_id, &artifact.id)?)?;
        if artifact.size > 0 {
            snapshot.nonempty_artifact_count += 1;
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0u8; 1];
            file.read_exact(&mut last)?;
            if last[0] != b'\n' {
                snapshot.nonempty_without_final_lf_count += 1;
            }
        }
        snapshot.byte_termination_checked_artifact_count += 1;
    }
    for artifact in retained {
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

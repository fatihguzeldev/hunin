use crate::{
    publication::PublicationRequest,
    storage::{ArtifactByteLimit, Store},
};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    io::{BufRead, Read, Seek, SeekFrom},
    path::Path,
    time::Instant,
};

const PRODUCER_DESCRIPTOR_LIMIT: usize = 64;
const PRODUCER_ID_BYTE_LIMIT: usize = 256;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RejectionStage {
    Budget,
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
    budget: BudgetReport,
    raw_retention: RawRetention,
    limitations: [&'static str; 4],
}

#[derive(Serialize)]
struct RawRetention {
    maximum_file_bytes: u64,
    maximum_read_bytes: u64,
    read_bytes: u64,
}

impl Default for RawRetention {
    fn default() -> Self {
        Self {
            maximum_file_bytes: super::retention::Limits::default().file_bytes,
            maximum_read_bytes: 1024 * 1024 * 1024,
            read_bytes: 0,
        }
    }
}

#[derive(Debug)]
struct DeadlineElapsed;
impl std::fmt::Display for DeadlineElapsed {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str("SDK finishing checkpoint deadline elapsed")
    }
}
impl std::error::Error for DeadlineElapsed {}

fn checkpoint(deadline: Option<Instant>) -> Result<()> {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(DeadlineElapsed.into());
    }
    Ok(())
}

#[derive(Clone, Copy, Serialize)]
struct SpoolBudget {
    max_directory_entries: usize,
    max_spool_files: usize,
    max_read_bytes: u64,
    max_line_bytes: u64,
    max_records: usize,
}

impl Default for SpoolBudget {
    fn default() -> Self {
        Self {
            max_directory_entries: 4096,
            max_spool_files: 1024,
            max_read_bytes: 64 * 1024 * 1024,
            max_line_bytes: 1024 * 1024,
            max_records: 100_000,
        }
    }
}

#[derive(Default, Serialize)]
struct SpoolWork {
    directory_entries: usize,
    spool_files: usize,
    read_bytes: u64,
    max_physical_line_bytes: u64,
    nonblank_records: usize,
}

#[derive(Serialize)]
struct BudgetReport {
    name: &'static str,
    limits: SpoolBudget,
    work: SpoolWork,
    exceeded: Option<BudgetExceeded>,
}

#[derive(Clone, Debug, Serialize)]
struct BudgetExceeded {
    limit: &'static str,
    maximum: u64,
    observed: u64,
}

impl std::fmt::Display for BudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SDK spool budget {} exceeded: observed {} > maximum {}",
            self.limit, self.observed, self.maximum
        )
    }
}

impl std::error::Error for BudgetExceeded {}

impl BudgetReport {
    fn check(&mut self, limit: &'static str, observed: u64, maximum: u64) -> Result<()> {
        if observed > maximum {
            let error = BudgetExceeded {
                limit,
                maximum,
                observed,
            };
            self.exceeded = Some(error.clone());
            return Err(error.into());
        }
        Ok(())
    }
}

impl Default for BudgetReport {
    fn default() -> Self {
        Self {
            name: "sdk-spool-decode-v1",
            limits: SpoolBudget::default(),
            work: SpoolWork::default(),
            exceeded: None,
        }
    }
}

impl Default for SpoolEvidence {
    fn default() -> Self {
        Self {
            scope: "retained_sdk_spool_snapshot",
            completion: DeliveryKnowledge::Unknown,
            loss: DeliveryKnowledge::Unknown,
            spool_snapshot: SpoolSnapshot::default(),
            budget: BudgetReport::default(),
            raw_retention: RawRetention::default(),
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
                "Raw retention and decode budgets bound separate reads; the supplied finishing deadline is checked through staging, decoding and publication, without preempting one filesystem or SQLite operation",
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
    ingest_with_budget(store, run_id, directory, SpoolBudget::default())
}

pub(super) fn ingest_controlled(
    store: &Store,
    run_id: &str,
    directory: &Path,
    deadline: Option<Instant>,
) -> IngestionOutcome {
    if deadline.is_none() {
        return ingest(store, run_id, directory);
    }
    ingest_with_policy(
        store,
        run_id,
        directory,
        SpoolBudget::default(),
        RawRetention::default(),
        deadline,
    )
}

fn ingest_with_budget(
    store: &Store,
    run_id: &str,
    directory: &Path,
    limits: SpoolBudget,
) -> IngestionOutcome {
    ingest_with_policy(
        store,
        run_id,
        directory,
        limits,
        RawRetention::default(),
        None,
    )
}

fn ingest_with_policy(
    store: &Store,
    run_id: &str,
    directory: &Path,
    limits: SpoolBudget,
    raw_retention: RawRetention,
    deadline: Option<Instant>,
) -> IngestionOutcome {
    let mut evidence = SpoolEvidence::default();
    evidence.budget.limits = limits;
    evidence.raw_retention = raw_retention;
    let records = match read(
        store,
        run_id,
        directory,
        &mut evidence.spool_snapshot,
        &mut evidence.budget,
        &mut evidence.raw_retention,
        deadline,
    ) {
        Ok(records) => records,
        Err(error) => {
            return IngestionOutcome {
                status: IngestionStatus::Rejected {
                    stage: if error.is::<BudgetExceeded>()
                        || error.is::<DeadlineElapsed>()
                        || error.is::<ArtifactByteLimit>()
                    {
                        RejectionStage::Budget
                    } else {
                        RejectionStage::Decode
                    },
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
    let status = publish_records(store, run_id, &records, &mut evidence, deadline);
    IngestionOutcome { status, evidence }
}

fn publish_records(
    store: &Store,
    run_id: &str,
    records: &[Value],
    evidence: &mut SpoolEvidence,
    deadline: Option<Instant>,
) -> IngestionStatus {
    match store.publish_result(PublicationRequest {
        run_id,
        source_key: "sdk-spool-v1",
        input_identity: "sdk-spool-v1",
        records,
        metadata: None,
        deadline,
    }) {
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
    }
}

#[cfg(test)]
#[path = "sdk/budget_tests.rs"]
mod budget_tests;

fn read(
    store: &Store,
    run_id: &str,
    directory: &Path,
    snapshot: &mut SpoolSnapshot,
    budget: &mut BudgetReport,
    raw_retention: &mut RawRetention,
    deadline: Option<Instant>,
) -> Result<Vec<Value>> {
    checkpoint(deadline)?;
    let limits = budget.limits;
    let mut records = Vec::new();
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        checkpoint(deadline)?;
        budget.work.directory_entries += 1;
        budget.check(
            "directory_entries",
            budget.work.directory_entries as u64,
            limits.max_directory_entries as u64,
        )?;
        let path = entry?.path();
        if !path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            continue;
        }
        budget.work.spool_files += 1;
        budget.check(
            "spool_files",
            budget.work.spool_files as u64,
            limits.max_spool_files as u64,
        )?;
        paths.push(path);
    }
    paths.sort();
    let mut retained = Vec::new();
    for path in paths {
        checkpoint(deadline)?;
        let maximum = raw_retention.maximum_file_bytes.min(
            raw_retention
                .maximum_read_bytes
                .saturating_sub(raw_retention.read_bytes),
        );
        let artifact = store.add_artifact_controlled(
            run_id,
            &path,
            "hunin-jsonl",
            maximum,
            &mut raw_retention.read_bytes,
            || checkpoint(deadline),
        )?;
        retained.push(artifact);
        snapshot.retained_artifact_count += 1;
    }
    // Snapshot every discovered spool before decoding any of them. A rejected
    // prefix must not hide later raw evidence or imply a complete inventory.
    snapshot.inventory_retained = true;
    for artifact in &retained {
        checkpoint(deadline)?;
        let mut file = fs::File::open(store.artifact_path(run_id, &artifact.id)?)?;
        if artifact.size > 0 {
            snapshot.nonempty_artifact_count += 1;
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0u8; 1];
            file.read_exact(&mut last)?;
            budget.work.read_bytes += 1;
            checkpoint(deadline)?;
            budget.check("read_bytes", budget.work.read_bytes, limits.max_read_bytes)?;
            if last[0] != b'\n' {
                snapshot.nonempty_without_final_lf_count += 1;
            }
        }
        snapshot.byte_termination_checked_artifact_count += 1;
    }
    for artifact in retained {
        checkpoint(deadline)?;
        let retained = store.artifact_path(run_id, &artifact.id)?;
        let before_file = budget.work.read_bytes;
        // One sentinel byte distinguishes exact EOF from a budget overrun.
        // Take bounds actual reads, even when metadata or a line is misleading.
        let allowance = limits
            .max_read_bytes
            .saturating_sub(before_file)
            .saturating_add(1);
        let mut reader = std::io::BufReader::new(fs::File::open(retained)?.take(allowance));
        let mut line = Vec::new();
        let mut index = 0;
        loop {
            checkpoint(deadline)?;
            line.clear();
            let result = Read::take(&mut reader, limits.max_line_bytes.saturating_add(1))
                .read_until(b'\n', &mut line);
            // Include BufReader prefetch in actual work, not only parsed bytes.
            budget.work.read_bytes =
                before_file.saturating_add(allowance - reader.get_ref().limit());
            budget.check("read_bytes", budget.work.read_bytes, limits.max_read_bytes)?;
            let count = result.with_context(|| {
                format!("read SDK artifact {} line {}", artifact.name, index + 1)
            })?;
            checkpoint(deadline)?;
            budget.work.max_physical_line_bytes =
                budget.work.max_physical_line_bytes.max(count as u64);
            budget.check("line_bytes", count as u64, limits.max_line_bytes)?;
            if count == 0 {
                break;
            }
            index += 1;
            let line = std::str::from_utf8(&line).with_context(|| {
                format!("SDK artifact {} line {} is not UTF-8", artifact.name, index)
            })?;
            if !line.trim().is_empty() {
                budget.work.nonblank_records += 1;
                budget.check(
                    "records",
                    budget.work.nonblank_records as u64,
                    limits.max_records as u64,
                )?;
                records.push(
                    serde_json::from_str(line).with_context(|| {
                        format!("SDK artifact {} line {}", artifact.name, index)
                    })?,
                );
                checkpoint(deadline)?;
            }
        }
    }
    checkpoint(deadline)?;
    Ok(records)
}

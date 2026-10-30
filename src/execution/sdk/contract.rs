//! SDK spool outcome, retained evidence and counted-work contracts.
use anyhow::Result;
use serde::Serialize;

pub(super) const PRODUCER_DESCRIPTOR_LIMIT: usize = 64;
pub(super) const PRODUCER_ID_BYTE_LIMIT: usize = 256;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RejectionStage {
    Budget,
    Decode,
    Acceptance,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum IngestionStatus {
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
pub(in crate::execution) struct IngestionOutcome {
    #[serde(flatten)]
    pub(super) status: IngestionStatus,
    pub(super) evidence: SpoolEvidence,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DeliveryKnowledge {
    #[default]
    Unknown,
}

#[derive(Default, Serialize)]
pub(super) struct SpoolSnapshot {
    pub(super) inventory_retained: bool,
    pub(super) retained_artifact_count: usize,
    pub(super) byte_termination_checked_artifact_count: usize,
    pub(super) nonempty_artifact_count: usize,
    pub(super) nonempty_without_final_lf_count: usize,
}

#[derive(Serialize)]
pub(super) struct ProducerDeclaration {
    pub(super) producer_id: String,
    /// The accepted JSON number's representation, without another numeric
    /// conversion through consumers such as JavaScript's Number.
    pub(super) pid_json_number: String,
}

#[derive(Serialize)]
pub(super) struct ProducerDeclarations {
    pub(super) source: &'static str,
    pub(super) entry_limit: usize,
    pub(super) producer_id_byte_limit: usize,
    pub(super) count: usize,
    pub(super) truncated: bool,
    pub(super) entries: Vec<ProducerDeclaration>,
}

#[derive(Serialize)]
pub(super) struct SpoolEvidence {
    pub(super) scope: &'static str,
    pub(super) completion: DeliveryKnowledge,
    pub(super) loss: DeliveryKnowledge,
    pub(super) spool_snapshot: SpoolSnapshot,
    pub(super) accepted_producer_declarations: ProducerDeclarations,
    pub(super) budget: BudgetReport,
    pub(super) raw_retention: RawRetention,
    pub(super) limitations: [&'static str; 4],
}

#[derive(Serialize)]
pub(super) struct RawRetention {
    pub(super) maximum_file_bytes: u64,
    pub(super) maximum_read_bytes: u64,
    pub(super) read_bytes: u64,
}

impl Default for RawRetention {
    fn default() -> Self {
        Self {
            maximum_file_bytes: crate::execution::retention::Limits::default().file_bytes,
            maximum_read_bytes: 1024 * 1024 * 1024,
            read_bytes: 0,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct SpoolBudget {
    pub(super) max_directory_entries: usize,
    pub(super) max_spool_files: usize,
    pub(super) max_read_bytes: u64,
    pub(super) max_line_bytes: u64,
    pub(super) max_records: usize,
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
pub(super) struct SpoolWork {
    pub(super) directory_entries: usize,
    pub(super) spool_files: usize,
    pub(super) read_bytes: u64,
    pub(super) max_physical_line_bytes: u64,
    pub(super) nonblank_records: usize,
}

#[derive(Serialize)]
pub(super) struct BudgetReport {
    pub(super) name: &'static str,
    pub(super) limits: SpoolBudget,
    pub(super) work: SpoolWork,
    pub(super) exceeded: Option<BudgetExceeded>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct BudgetExceeded {
    pub(super) limit: &'static str,
    pub(super) maximum: u64,
    pub(super) observed: u64,
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
    pub(super) fn check(&mut self, limit: &'static str, observed: u64, maximum: u64) -> Result<()> {
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

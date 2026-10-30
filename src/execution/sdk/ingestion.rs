//! All-or-none SDK acceptance and projection of accepted producer declarations.
use super::{
    IngestionOutcome,
    contract::{
        BudgetExceeded, IngestionStatus, PRODUCER_DESCRIPTOR_LIMIT, PRODUCER_ID_BYTE_LIMIT,
        ProducerDeclaration, RawRetention, RejectionStage, SpoolBudget, SpoolEvidence,
    },
    spool::{self, DeadlineElapsed},
};
use crate::{
    publication::PublicationRequest,
    storage::{ArtifactByteLimit, Store},
};
use serde_json::Value;
use std::{collections::BTreeSet, path::Path, time::Instant};

/// Decode immutable retained SDK bytes and atomically accept the whole result.
pub(in crate::execution) fn ingest_controlled(
    store: &Store,
    run_id: &str,
    directory: &Path,
    deadline: Option<Instant>,
) -> IngestionOutcome {
    ingest_with_policy(
        store,
        run_id,
        directory,
        SpoolBudget::default(),
        RawRetention::default(),
        deadline,
    )
}

pub(super) fn ingest_with_policy(
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
    let records = match spool::read(
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

pub(super) fn publish_records(
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

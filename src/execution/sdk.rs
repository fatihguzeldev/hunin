//! Recording-owned SDK spool contracts, decoding and atomic ingestion.
mod contract;
mod ingestion;
mod spool;

pub(super) use contract::IngestionOutcome;
pub(super) use ingestion::ingest_controlled;

#[cfg(test)]
mod tests;

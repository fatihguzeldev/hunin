# Observation contract

`record.schema.json` is the authoritative v1 external observation schema. Producers,
importers, SDKs and analyzers submit records through ingestion, never private SQL.
JSON Schema is checked first; run membership, identities and references are then
checked atomically for the entire submitted result set.

`producer_id` identifies one producer/process lifetime, not a reusable PID. Record
IDs must be unique within a run. Repeating an unchanged accepted input does not
publish it twice. References (`parent_id`, `context_id`, relationship endpoints and
evidence) address records in the same run, including records in the same batch.

`timestamp_us` and `duration_us` use integer microseconds, capped at JavaScript's
safe integer range. The default timestamp clock is Unix epoch. A source using an
unaligned clock must set `attributes.clock` and `attributes.clock_alignment` to
`unmapped`; those records remain queryable but are excluded from the shared epoch
timeline. No linear wall-time alignment is inferred from aggregate CPU progress.

Stacks are ordered root to leaf. A sample's weight and weight unit are explicit:
samples, microseconds, bytes or count. Sampling is neither a complete call history
nor a measurement of every function's elapsed duration. Process metrics retain
process scope even when a context is active. Temporal overlap alone does not
create causal links or ownership.

`basis` distinguishes observed, sampled, explicit SDK and derived information.
Derived results retain their original inputs, analyzer identity/version and
declared applicability at the result-set level. Raw artifacts remain immutable.
Unknown frames and unresolved relationships must remain visible.

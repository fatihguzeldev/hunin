//! Decode retained JSONL observations without changing their recording owner.
use super::output::RecordBatch;
use anyhow::{Context, Result, bail};
use serde_json::Value;

pub(super) fn decode_into(raw: &[u8], run_id: &str, records: &mut RecordBatch<'_>) -> Result<()> {
    for (line, text) in std::str::from_utf8(raw)?.lines().enumerate() {
        if text.trim().is_empty() {
            continue;
        }
        let record: Value =
            serde_json::from_str(text).with_context(|| format!("JSONL line {}", line + 1))?;
        if record["run_id"].as_str() != Some(run_id) {
            bail!("JSONL record belongs to another run");
        }
        records.push(record)?;
    }
    Ok(())
}

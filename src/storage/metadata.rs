//! Selected metadata pages are bounded before Rust payload copies or JSON decode.
use anyhow::{Context, Result};
use rusqlite::{Connection, Params};
use serde::de::DeserializeOwned;
use std::fmt;

const MAXIMUM_ROWS: usize = 501;
const MAXIMUM_ROW_BYTES: usize = 8 * 1024 * 1024;
const MAXIMUM_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataPageDimension {
    Rows,
    RowBytes,
    Bytes,
}
impl MetadataPageDimension {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rows => "rows",
            Self::RowBytes => "row_bytes",
            Self::Bytes => "selected_bytes",
        }
    }
}

/// Actual selected SQL TEXT exceeded the metadata page policy. Limits cover
/// 501 rows (including the pagination sentinel), 8 MiB per row and 128 MiB total;
/// they are serialized-work bounds, not a bound on SQLite or decoded JSON heap.
#[derive(Clone, Debug)]
pub struct MetadataPageBudgetExceeded {
    pub dimension: MetadataPageDimension,
    pub limit: u64,
    pub observed: u64,
}
impl fmt::Display for MetadataPageBudgetExceeded {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            output,
            "metadata page {} budget exceeded: observed {}, limit {}",
            self.dimension.as_str(),
            self.observed,
            self.limit
        )
    }
}
impl std::error::Error for MetadataPageBudgetExceeded {}

struct PageBudget {
    rows: usize,
    bytes: usize,
    maximum_rows: usize,
    maximum_row_bytes: usize,
    maximum_bytes: usize,
}
impl Default for PageBudget {
    fn default() -> Self {
        Self {
            rows: 0,
            bytes: 0,
            maximum_rows: MAXIMUM_ROWS,
            maximum_row_bytes: MAXIMUM_ROW_BYTES,
            maximum_bytes: MAXIMUM_BYTES,
        }
    }
}
impl PageBudget {
    fn decode<T: DeserializeOwned>(&mut self, text: &str) -> Result<T> {
        self.rows = self.rows.saturating_add(1);
        within(MetadataPageDimension::Rows, self.maximum_rows, self.rows)?;
        within(
            MetadataPageDimension::RowBytes,
            self.maximum_row_bytes,
            text.len(),
        )?;
        self.bytes = self.bytes.saturating_add(text.len());
        within(MetadataPageDimension::Bytes, self.maximum_bytes, self.bytes)?;
        Ok(serde_json::from_str(text)?)
    }
}

fn within(dimension: MetadataPageDimension, limit: usize, observed: usize) -> Result<()> {
    if observed > limit {
        return Err(MetadataPageBudgetExceeded {
            dimension,
            limit: limit as u64,
            observed: observed as u64,
        }
        .into());
    }
    Ok(())
}

/// Call within one read transaction: sort scalar row keys, then look up only
/// the selected payloads. SQLite may still parse a document's JSON order key;
/// neither that unit nor its sorting work has a hard time or heap guarantee.
pub(super) fn read_page<T: DeserializeOwned>(
    connection: &Connection,
    selection: &str,
    parameters: impl Params,
    payload: &str,
) -> Result<Vec<T>> {
    let mut selected = connection.prepare(selection)?;
    let mut payload = connection.prepare(payload)?;
    let mut selected = selected.query(parameters)?;
    let mut budget = PageBudget::default();
    let mut values = Vec::new();
    while let Some(key) = selected.next()? {
        let rowid: i64 = key.get(0)?;
        let mut rows = payload.query([rowid])?;
        let row = rows.next()?.context("selected metadata row disappeared")?;
        // Borrow SQLite's TEXT buffer. No String/Value copy precedes the actual
        // UTF-8 byte checks, including the limit+1 pagination sentinel.
        let text = row.get_ref(0)?.as_str()?;
        values.push(budget.decode(text)?);
    }
    Ok(values)
}

#[cfg(test)]
mod tests;

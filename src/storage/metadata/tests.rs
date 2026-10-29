use super::*;
use serde_json::{Value, json};

fn budget(rows: usize, row_bytes: usize, bytes: usize) -> PageBudget {
    PageBudget {
        maximum_rows: rows,
        maximum_row_bytes: row_bytes,
        maximum_bytes: bytes,
        ..Default::default()
    }
}

fn exceeded(error: anyhow::Error, dimension: MetadataPageDimension, limit: u64, observed: u64) {
    let error = error.downcast_ref::<MetadataPageBudgetExceeded>().unwrap();
    assert_eq!(error.dimension, dimension);
    assert_eq!(error.limit, limit);
    assert_eq!(error.observed, observed);
}

#[test]
fn metadata_byte_limits_precede_decode_and_count_actual_utf8_not_characters() {
    let mut row_budget = budget(3, 3, 100);
    // Four actual UTF-8 bytes, although only three characters; malformed JSON
    // deliberately makes the typed origin prove the ordering of the checks.
    exceeded(
        row_budget.decode::<Value>("éxx").unwrap_err(),
        MetadataPageDimension::RowBytes,
        3,
        4,
    );
    let mut page_budget = budget(3, 100, 6);
    assert_eq!(page_budget.decode::<Value>("\"é\"").unwrap(), json!("é"));
    exceeded(
        page_budget.decode::<Value>("bad").unwrap_err(),
        MetadataPageDimension::Bytes,
        6,
        7,
    );
}

#[test]
fn the_sentinel_row_is_accounted_before_decode() {
    let mut budget = budget(1, 100, 100);
    assert_eq!(budget.decode::<Value>("null").unwrap(), Value::Null);
    exceeded(
        budget.decode::<Value>("invalid JSON").unwrap_err(),
        MetadataPageDimension::Rows,
        1,
        2,
    );
}

#[test]
fn exact_byte_limits_accept_the_complete_legacy_value() {
    let value = json!({"extension":{"unit":"unknown","pid":1e30},"text":"é"});
    let text = serde_json::to_string(&value).unwrap();
    let mut budget = budget(1, text.len(), text.len());
    assert_eq!(budget.decode::<Value>(&text).unwrap(), value);
}

//! Encoded result budgets are separate from query and transport work budgets.
use anyhow::Result;
use serde_json::Value;
use std::{fmt, io};

pub(super) const MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct BudgetExceeded {
    pub maximum: usize,
    pub observed: usize,
}
impl fmt::Display for BudgetExceeded {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            output,
            "encoded operation result exceeds its {} byte budget (observed at least {}); narrow the query or request fewer rows",
            self.maximum, self.observed
        )
    }
}
impl std::error::Error for BudgetExceeded {}

struct Counter {
    maximum: usize,
    bytes: usize,
    exceeded: Option<usize>,
}
impl io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let observed = self.bytes.saturating_add(bytes.len());
        if observed > self.maximum {
            self.exceeded = Some(observed);
            return Err(io::Error::other("encoded result budget exceeded"));
        }
        self.bytes = observed;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn check(value: &Value) -> Result<()> {
    check_limit(value, MAX_RESULT_BYTES)
}

fn check_limit(value: &Value, maximum: usize) -> Result<()> {
    let mut counter = Counter {
        maximum,
        bytes: 0,
        exceeded: None,
    };
    // Count serializer output without allocating an encoded response copy.
    if let Err(error) = serde_json::to_writer(&mut counter, value) {
        if let Some(observed) = counter.exceeded {
            return Err(BudgetExceeded { maximum, observed }.into());
        }
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_budget_counts_utf8_and_json_escapes_at_the_exact_boundary() {
        let result = serde_json::json!({"text":"Türkçe\n\"\\\u{0001}","rows":[1,2,3]});
        let before = result.clone();
        let exact = serde_json::to_vec(&result).unwrap().len();
        check_limit(&result, exact).unwrap();
        let error = check_limit(&result, exact - 1).unwrap_err();
        assert!(error.downcast_ref::<BudgetExceeded>().is_some());
        assert_eq!(result, before);
    }

    #[tokio::test]
    async fn oversized_read_returns_a_typed_mcp_failure_with_stored_data_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let store = crate::storage::Store::open(directory.path()).unwrap();
        for index in 0..9 {
            let id = format!("row-{index}");
            store
                .save_document(
                    "scenarios",
                    &id,
                    &serde_json::json!({"id":id,"payload":"x".repeat(2*1024*1024)}),
                )
                .unwrap();
        }
        let operations = crate::operations::Operations::new(store.clone());
        let response = crate::interfaces::mcp::handle(&operations, &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"scenarios.list","arguments":{}}}), true).await;
        let result = &response["result"];
        assert_eq!(result["isError"], true);
        assert!(result.get("structuredContent").is_none());
        let failure = &result["_meta"]["hunin/operation-error"];
        assert_eq!(failure["code"], "work_budget_exceeded");
        assert_eq!(failure["budget"]["scope"], "operation_result");
        assert_eq!(failure["budget"]["dimension"], "encoded_result_bytes");
        assert_eq!(failure["budget"]["limit"], MAX_RESULT_BYTES);
        assert!(failure["budget"]["observed"].as_u64().unwrap() > MAX_RESULT_BYTES as u64);
        assert_eq!(
            store.document("scenarios", "row-0").unwrap()["payload"]
                .as_str()
                .unwrap()
                .len(),
            2 * 1024 * 1024
        );
    }
}

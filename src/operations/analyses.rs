//! Analyzer-result listing, recovery and selected-result projection.
use super::{Operations, metadata_page};
use anyhow::{Context, Result};
use serde_json::{Value, json};

impl Operations {
    pub(super) fn analysis_results(
        &self,
        params: &Value,
        limit: usize,
        cursor: usize,
    ) -> Result<Value> {
        self.analyses.recover()?;
        let store = self.engine.store();
        let run_id = params["run_id"].as_str().context("run_id required")?;
        store.run(run_id)?;
        let mut results = if let Some(execution_id) = params["execution_id"].as_str() {
            if cursor == 0 {
                store
                    .document_for_run("analysis-results", execution_id, run_id)?
                    .into_iter()
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            store.documents_page("analysis-results", Some(run_id), limit + 1, cursor)?
        };
        for row in &mut results {
            let selected = store.selected_result_id(
                run_id,
                &format!("analysis:{}", row["analysis_id"].as_str().unwrap_or("")),
            )?;
            let result_id = row["result_set_id"].as_str().or_else(|| row["id"].as_str());
            row["selected"] = json!(result_id.is_some() && selected.as_deref() == result_id);
        }
        Ok(metadata_page("results", results, limit, cursor))
    }
}

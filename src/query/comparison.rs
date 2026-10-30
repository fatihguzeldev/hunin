//! Comparison owns a cumulative read budget across two selected snapshots.
use crate::storage::{
    QuerySnapshot, ReadBudgetExceeded, ReadDimension, ReadLimits, ReadUsage, Store,
};
use anyhow::Result;
use serde_json::Value;

fn remaining(mut limits: ReadLimits, usage: &ReadUsage) -> ReadLimits {
    limits.maximum_rows = limits.maximum_rows.saturating_sub(usage.rows);
    limits.maximum_bytes = limits
        .maximum_bytes
        .saturating_sub(usage.bytes.saturating_add(usage.identity_bytes));
    limits.maximum_result_sets = limits.maximum_result_sets.saturating_sub(usage.result_sets);
    limits.maximum_sqlite_steps = limits
        .maximum_sqlite_steps
        .saturating_sub(usage.sqlite_steps_approx);
    limits
}

type MaterializedSnapshot = QuerySnapshot<Vec<Value>>;

pub(crate) fn comparison_inputs(
    store: &Store,
    before: &str,
    after: &str,
    limits: ReadLimits,
    checkpoint: impl FnMut() -> Result<()> + Send + Clone + 'static,
) -> Result<(MaterializedSnapshot, MaterializedSnapshot)> {
    let read = |id: &str, limits: ReadLimits, checkpoint| {
        store.with_query_snapshot(id, limits, checkpoint, |port| {
            let mut records = vec![];
            port.visit(&["sample", "metric"], |text| {
                records.push(serde_json::from_str(text)?);
                Ok(())
            })?;
            Ok(records)
        })
    };
    let before = read(before, limits.clone(), checkpoint.clone())?;
    let after = read(after, remaining(limits.clone(), &before.usage), checkpoint)
        .map_err(|error| cumulative_error(error, &limits, &before.usage))?;
    Ok((before, after))
}

fn cumulative_error(error: anyhow::Error, limits: &ReadLimits, usage: &ReadUsage) -> anyhow::Error {
    let Some(budget) = error.downcast_ref::<ReadBudgetExceeded>() else {
        return error;
    };
    let (prior, limit) = match budget.dimension {
        ReadDimension::Rows => (usage.rows as u64, limits.maximum_rows as u64),
        ReadDimension::Bytes => (
            usage.bytes.saturating_add(usage.identity_bytes) as u64,
            limits.maximum_bytes as u64,
        ),
        ReadDimension::ResultSets => (usage.result_sets as u64, limits.maximum_result_sets as u64),
        ReadDimension::SqliteSteps => (usage.sqlite_steps_approx, limits.maximum_sqlite_steps),
        ReadDimension::RecordBytes => return error,
    };
    ReadBudgetExceeded {
        dimension: budget.dimension,
        limit,
        observed: prior.saturating_add(budget.observed),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn comparisons_share_one_read_budget_across_both_selected_snapshots() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        store
            .save_run(&crate::model::runs::Run {
                id: "recording".into(),
                name: "comparison read control".into(),
                status: "completed".into(),
                command: vec![],
                cwd: "/fixture".into(),
                runtime: "fixture".into(),
                started_at_us: 1,
                finished_at_us: Some(2),
                exit_code: Some(0),
                diagnostics: vec![],
                source: json!({}),
                correctness: json!({}),
            })
            .unwrap();
        store.accept("recording","input",&[json!({"schema_version":1,"kind":"sample","id":"sample","run_id":"recording","producer_id":"fixture","pid":1,"tid":"thread","timestamp_us":1,"basis":"sampled","weight":1,"weight_unit":"samples","stack":[{"function":"work"}]})]).unwrap();
        let error = comparison_inputs(
            &store,
            "recording",
            "recording",
            ReadLimits {
                maximum_rows: 1,
                ..Default::default()
            },
            || Ok(()),
        )
        .unwrap_err();
        let error = error.downcast_ref::<ReadBudgetExceeded>().unwrap();
        assert_eq!(error.dimension, ReadDimension::Rows);
        assert_eq!(error.limit, 1);
        assert_eq!(error.observed, 2);
        let (before, after) = comparison_inputs(
            &store,
            "recording",
            "recording",
            ReadLimits {
                maximum_rows: 2,
                ..Default::default()
            },
            || Ok(()),
        )
        .unwrap();
        assert_eq!(before.value, after.value);
        assert_eq!(before.usage.rows + after.usage.rows, 2);
    }
}

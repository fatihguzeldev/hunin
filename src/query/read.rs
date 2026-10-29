//! Store-backed query entry points share one selected-observation snapshot.
use super::{QueryLimits, WorkBudget};
use crate::storage::{
    QuerySnapshot, ReadBudgetExceeded, ReadDimension, ReadLimits, ReadUsage, Store,
};
use anyhow::Result;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;

pub enum RecordingQuery<'a> {
    Summary,
    Hotspots { unit: &'a str, limit: usize },
    Tree(&'a Value),
    Timeline(&'a Value),
    Resources(&'a Value),
}

#[derive(Default, Deserialize)]
struct SummaryRecord {
    #[serde(default)]
    kind: Value,
    #[serde(default)]
    weight_unit: Value,
    #[serde(default)]
    weight: Value,
}

#[derive(Default)]
struct Summary {
    selected: Option<String>,
    samples: usize,
    spans: usize,
    metrics: usize,
    relations: usize,
    total: f64,
    units: HashSet<String>,
}
impl Summary {
    fn add(&mut self, text: &str, work: &mut WorkBudget) -> Result<()> {
        work.check()?;
        let record: SummaryRecord = serde_json::from_str(text)?;
        match record.kind.as_str() {
            Some("sample") => {
                let selected = self
                    .selected
                    .get_or_insert_with(|| record.weight_unit.as_str().unwrap_or("samples").into());
                self.samples += 1;
                if record.weight_unit == *selected {
                    self.total += record.weight.as_f64().unwrap_or(0.0);
                }
                if let Some(unit) = record.weight_unit.as_str()
                    && !self.units.contains(unit)
                {
                    work.entry()?;
                    self.units.insert(unit.to_owned());
                }
            }
            Some("span") => self.spans += 1,
            Some("metric") => self.metrics += 1,
            Some("relation") => self.relations += 1,
            _ => {}
        }
        work.check()
    }
    fn finish(self) -> Value {
        json!({"sample_count":self.samples,"total_weight":self.total,"weight_unit":self.selected.unwrap_or_else(||"samples".into()),"span_count":self.spans,"metric_count":self.metrics,"relation_count":self.relations,"diagnostics":if self.units.len()>1 {vec!["Multiple sample units exist; query each unit separately."]}else{vec![]}})
    }
}

pub fn read_recording(
    store: &Store,
    run_id: &str,
    query: RecordingQuery<'_>,
    limits: QueryLimits,
    checkpoint: impl FnMut() -> Result<()> + Send + 'static,
) -> Result<QuerySnapshot<Value>> {
    store.with_query_snapshot(run_id, limits.read.clone(), checkpoint, |port| {
        let mut work = WorkBudget::new(limits, port.checkpointer());
        let value = match query {
            RecordingQuery::Summary => {
                let mut summary = Summary::default();
                port.visit(&[], |text| summary.add(text, &mut work))?;
                summary.finish()
            }
            RecordingQuery::Hotspots { unit, limit } => {
                let mut page = super::profiles::HotspotAccumulator::profile(unit);
                port.visit(&["sample"], |text| {
                    page.add(&serde_json::from_str(text)?, &mut work)
                })?;
                page.finish(limit, &mut work)?.result
            }
            RecordingQuery::Tree(params) => {
                let mut page = super::profiles::TreeAccumulator::new(params);
                port.visit(&["sample"], |text| {
                    page.add(&serde_json::from_str(text)?, &mut work)
                })?;
                page.finish(params, &mut work)?
            }
            RecordingQuery::Timeline(params) => super::timeline::read(port, params, &mut work)?,
            RecordingQuery::Resources(params) => {
                let mut records = Vec::new();
                port.visit(&["metric"], |text| {
                    records.push(serde_json::from_str(text)?);
                    Ok(())
                })?;
                super::resources::resources_page_checked(&records, params, &mut work)?
            }
        };
        work.check()?;
        Ok(value)
    })
}

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

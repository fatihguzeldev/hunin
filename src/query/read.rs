//! Store-backed query entry points share one selected-observation snapshot.
use super::{QueryLimits, WorkBudget, summary::Summary};
use crate::storage::{QuerySnapshot, Store};
use anyhow::Result;
use serde_json::Value;

pub enum RecordingQuery<'a> {
    Summary,
    Hotspots { unit: &'a str, limit: usize },
    Tree(&'a Value),
    Timeline(&'a Value),
    Resources(&'a Value),
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

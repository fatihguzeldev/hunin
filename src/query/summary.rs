//! One summary accumulator for retained JSON rows and in-memory observations.
use super::WorkBudget;
use anyhow::Result;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;

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
pub(super) struct Summary {
    selected: Option<String>,
    samples: usize,
    spans: usize,
    metrics: usize,
    relations: usize,
    total: f64,
    units: HashSet<String>,
}
impl Summary {
    pub(super) fn add(&mut self, text: &str, work: &mut WorkBudget) -> Result<()> {
        work.check()?;
        let record: SummaryRecord = serde_json::from_str(text)?;
        self.add_fields(&record.kind, &record.weight_unit, &record.weight, work)
    }

    fn add_fields(
        &mut self,
        kind: &Value,
        weight_unit: &Value,
        weight: &Value,
        work: &mut WorkBudget,
    ) -> Result<()> {
        match kind.as_str() {
            Some("sample") => {
                let selected = self
                    .selected
                    .get_or_insert_with(|| weight_unit.as_str().unwrap_or("samples").into());
                self.samples += 1;
                if *weight_unit == *selected {
                    self.total += weight.as_f64().unwrap_or(0.0);
                }
                if let Some(unit) = weight_unit.as_str()
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
    pub(super) fn finish(self) -> Value {
        json!({"sample_count":self.samples,"total_weight":self.total,"weight_unit":self.selected.unwrap_or_else(||"samples".into()),"span_count":self.spans,"metric_count":self.metrics,"relation_count":self.relations,"diagnostics":if self.units.len()>1 {vec!["Multiple sample units exist; query each unit separately."]}else{vec![]}})
    }
}

pub fn summary(records: &[Value]) -> Value {
    let mut work = WorkBudget::unlimited();
    let mut summary = Summary {
        // Retain the slice API's Iterator::sum initial value; the streamed
        // API keeps its existing default accumulator value.
        total: std::iter::empty::<f64>().sum(),
        ..Default::default()
    };
    for record in records {
        summary
            .add_fields(
                &record["kind"],
                &record["weight_unit"],
                &record["weight"],
                &mut work,
            )
            .expect("unlimited query work");
    }
    summary.finish()
}

#[cfg(test)]
mod tests;

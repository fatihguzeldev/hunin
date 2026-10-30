use super::{function_identity, number};
use crate::query::WorkBudget;
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

pub(crate) struct HotspotCoverage {
    pub(crate) result: Value,
    pub(crate) total_functions: usize,
}

pub fn hotspots(records: &[Value], requested_unit: &str, limit: usize) -> Value {
    let mut work = WorkBudget::unlimited();
    let mut page = HotspotAccumulator::new(requested_unit, function_identity);
    for record in records {
        page.add(record, &mut work).expect("unlimited query work");
    }
    page.finish(limit, &mut work)
        .expect("unlimited query work")
        .result
}

/// Comparison deliberately groups symbols across measured code versions.
/// Aggregate that identity before the rank limit so hidden versions do not
/// produce partial symbol totals. The ordinary profile page keeps code IDs.
pub(crate) fn comparison_hotspots_checked(
    records: &[Value],
    requested_unit: &str,
    limit: usize,
    work: &mut WorkBudget,
) -> Result<HotspotCoverage> {
    let mut page = HotspotAccumulator::new(requested_unit, |frame| {
        json!([frame["function"], frame["file"], frame["module"]]).to_string()
    });
    for record in records {
        page.add(record, work)?;
    }
    page.finish(limit, work)
}

pub(in crate::query) struct HotspotAccumulator {
    selected: Option<String>,
    functions: BTreeMap<String, Value>,
    total: f64,
    identity: fn(&Value) -> String,
}
impl HotspotAccumulator {
    fn new(requested_unit: &str, identity: fn(&Value) -> String) -> Self {
        Self {
            selected: (!requested_unit.is_empty()).then(|| requested_unit.to_owned()),
            functions: BTreeMap::new(),
            total: 0.0,
            identity,
        }
    }
    pub(in crate::query) fn profile(requested_unit: &str) -> Self {
        Self::new(requested_unit, function_identity)
    }
    pub(in crate::query) fn add(&mut self, r: &Value, work: &mut WorkBudget) -> Result<()> {
        work.check()?;
        if r["kind"] != "sample" {
            return Ok(());
        }
        let selected = self
            .selected
            .get_or_insert_with(|| r["weight_unit"].as_str().unwrap_or("samples").to_owned());
        if r["weight_unit"] != *selected {
            return Ok(());
        }
        let weight = number(r, "weight");
        self.total += weight;
        let Some(stack) = r["stack"].as_array() else {
            return Ok(());
        };
        let mut inclusive = HashSet::new();
        for (index, frame) in stack.iter().enumerate() {
            work.frame()?;
            let key = (self.identity)(frame);
            if !self.functions.contains_key(&key) {
                work.entry()?;
            }
            let row=self.functions.entry(key.clone()).or_insert_with(||json!({"function":frame["function"],"file":frame["file"],"line":frame["line"],"module":frame["module"],"code_id":frame["code_id"],"source_position_basis":"first observed sampled position","self_weight":0.0,"inclusive_weight":0.0,"weight_unit":selected}));
            if inclusive.insert(key) {
                row["inclusive_weight"] = json!(number(row, "inclusive_weight") + weight);
            }
            if index + 1 == stack.len() {
                row["self_weight"] = json!(number(row, "self_weight") + weight);
            }
        }
        Ok(())
    }
    pub(in crate::query) fn finish(
        self,
        limit: usize,
        work: &mut WorkBudget,
    ) -> Result<HotspotCoverage> {
        work.check()?;
        let mut rows: Vec<_> = self.functions.into_values().collect();
        rows.sort_by(|a, b| {
            number(b, "self_weight")
                .total_cmp(&number(a, "self_weight"))
                .then_with(|| a["function"].as_str().cmp(&b["function"].as_str()))
        });
        let total_functions = rows.len();
        rows.truncate(limit.min(1000));
        work.check()?;
        Ok(HotspotCoverage {
            result: json!({"hotspots":rows,"total_weight":self.total,"weight_unit":self.selected.unwrap_or_else(|| "samples".into())}),
            total_functions,
        })
    }
}

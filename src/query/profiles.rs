use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

fn number(v: &Value, field: &str) -> f64 {
    v[field].as_f64().unwrap_or(0.0)
}
fn unit(records: &[Value], requested: &str) -> String {
    if !requested.is_empty() {
        return requested.into();
    }
    records
        .iter()
        .find(|r| r["kind"] == "sample")
        .and_then(|r| r["weight_unit"].as_str())
        .unwrap_or("samples")
        .into()
}

fn function_identity(frame: &Value) -> String {
    json!([
        frame["function"],
        frame["file"],
        frame["module"],
        frame["code_id"]
    ])
    .to_string()
}

pub fn summary(records: &[Value]) -> Value {
    let selected = unit(records, "");
    let total: f64 = records
        .iter()
        .filter(|r| r["kind"] == "sample" && r["weight_unit"] == selected)
        .map(|r| number(r, "weight"))
        .sum();
    let units: HashSet<_> = records
        .iter()
        .filter(|r| r["kind"] == "sample")
        .filter_map(|r| r["weight_unit"].as_str())
        .collect();
    json!({"sample_count":records.iter().filter(|r|r["kind"]=="sample").count(),"total_weight":total,"weight_unit":selected,"span_count":records.iter().filter(|r|r["kind"]=="span").count(),"metric_count":records.iter().filter(|r|r["kind"]=="metric").count(),"relation_count":records.iter().filter(|r|r["kind"]=="relation").count(),"diagnostics":if units.len()>1 {vec!["Multiple sample units exist; query each unit separately."]}else{vec![]}})
}

pub fn hotspots(records: &[Value], requested_unit: &str, limit: usize) -> Value {
    let selected = unit(records, requested_unit);
    let mut functions: BTreeMap<String, Value> = BTreeMap::new();
    let mut total = 0.0;
    for r in records
        .iter()
        .filter(|r| r["kind"] == "sample" && r["weight_unit"] == selected)
    {
        let weight = number(r, "weight");
        total += weight;
        let Some(stack) = r["stack"].as_array() else {
            continue;
        };
        let mut inclusive = HashSet::new();
        for (index, frame) in stack.iter().enumerate() {
            let key = function_identity(frame);
            let row=functions.entry(key.clone()).or_insert_with(||json!({"function":frame["function"],"file":frame["file"],"line":frame["line"],"module":frame["module"],"code_id":frame["code_id"],"source_position_basis":"first observed sampled position","self_weight":0.0,"inclusive_weight":0.0,"weight_unit":selected}));
            if inclusive.insert(key) {
                row["inclusive_weight"] = json!(number(row, "inclusive_weight") + weight);
            }
            if index + 1 == stack.len() {
                row["self_weight"] = json!(number(row, "self_weight") + weight);
            }
        }
    }
    let mut rows: Vec<_> = functions.into_values().collect();
    rows.sort_by(|a, b| {
        number(b, "self_weight")
            .total_cmp(&number(a, "self_weight"))
            .then_with(|| a["function"].as_str().cmp(&b["function"].as_str()))
    });
    rows.truncate(limit.min(1000));
    json!({"hotspots":rows,"total_weight":total,"weight_unit":selected})
}

#[derive(Default)]
struct Tree {
    frame: Value,
    own: f64,
    total: f64,
    children: BTreeMap<String, Tree>,
}
impl Tree {
    fn add(&mut self, stack: &[Value], weight: f64) {
        self.total += weight;
        if let Some((frame, rest)) = stack.split_first() {
            self.children
                .entry(function_identity(frame))
                .or_insert_with(|| Tree {
                    frame: frame.clone(),
                    ..Default::default()
                })
                .add(rest, weight);
        } else {
            self.own += weight;
        }
    }
    fn value(self) -> Value {
        let mut children: Vec<Value> = self.children.into_values().map(Tree::value).collect();
        children.sort_by(|a, b| number(b, "total_weight").total_cmp(&number(a, "total_weight")));
        json!({"name":self.frame["function"],"file":self.frame["file"],"line":self.frame["line"],"module":self.frame["module"],"code_id":self.frame["code_id"],"source_position_basis":"first observed sampled position","self_weight":self.own,"total_weight":self.total,"children":children})
    }
}
pub fn tree(records: &[Value], requested_unit: &str) -> Value {
    let selected = unit(records, requested_unit);
    let mut tree = Tree::default();
    for r in records
        .iter()
        .filter(|r| r["kind"] == "sample" && r["weight_unit"] == selected)
    {
        if let Some(stack) = r["stack"].as_array() {
            tree.add(stack, number(r, "weight"));
        }
    }
    let v = tree.value();
    json!({"roots":v["children"],"weight_unit":selected})
}

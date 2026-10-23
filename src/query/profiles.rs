use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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
    depth_truncated: bool,
}
impl Tree {
    fn add(&mut self, stack: &[Value], weight: f64, remaining: usize) {
        self.total += weight;
        if remaining == 0 && !stack.is_empty() {
            self.depth_truncated = true;
            return;
        }
        if let Some((frame, rest)) = stack.split_first() {
            self.children
                .entry(function_identity(frame))
                .or_insert_with(|| Tree {
                    frame: frame.clone(),
                    ..Default::default()
                })
                .add(rest, weight, remaining - 1);
        } else {
            self.own += weight;
        }
    }
    fn value(self, budget: &mut usize, depth: usize) -> Option<Value> {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let id = frame_id(&self.frame);
        let count = self.children.len();
        let mut candidates = self.children.into_values().collect::<Vec<_>>();
        candidates.sort_by(|a, b| b.total.total_cmp(&a.total));
        let mut children = Vec::new();
        if depth > 0 {
            for child in candidates {
                if let Some(value) = child.value(budget, depth - 1) {
                    children.push(value);
                }
                if *budget == 0 {
                    break;
                }
            }
        }
        Some(
            json!({"id":id,"name":self.frame["function"],"file":self.frame["file"],"line":self.frame["line"],"module":self.frame["module"],"code_id":self.frame["code_id"],"source_position_basis":"first observed sampled position","self_weight":self.own,"total_weight":self.total,"child_count":count,"children_truncated":children.len()<count||self.depth_truncated,"depth_truncated":self.depth_truncated,"children":children}),
        )
    }
}
fn frame_id(frame: &Value) -> String {
    format!(
        "frame:{:x}",
        Sha256::digest(function_identity(frame).as_bytes())
    )
}

pub fn tree(records: &[Value], requested_unit: &str) -> Value {
    tree_page(records, &json!({"weight_unit":requested_unit,"limit":5000}))
}

pub fn tree_page(records: &[Value], params: &Value) -> Value {
    let selected = unit(records, params["weight_unit"].as_str().unwrap_or(""));
    let path = params["path"].as_array().cloned().unwrap_or_default();
    let mut tree = Tree::default();
    for record in records
        .iter()
        .filter(|r| r["kind"] == "sample" && r["weight_unit"] == selected)
    {
        if let Some(stack) = record["stack"].as_array() {
            if stack.len() >= path.len()
                && path
                    .iter()
                    .zip(stack)
                    .all(|(id, frame)| id.as_str() == Some(frame_id(frame).as_str()))
            {
                tree.add(&stack[path.len()..], number(record, "weight"), 128);
            }
        }
    }
    let total = tree.total;
    let own = tree.own;
    let mut roots = tree.children.into_values().collect::<Vec<_>>();
    roots.sort_by(|a, b| b.total.total_cmp(&a.total));
    let available = roots.len();
    let offset = params["cursor"]
        .as_str()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = params["limit"].as_u64().unwrap_or(1000).clamp(1, 5000) as usize;
    let depth = params["depth_limit"].as_u64().unwrap_or(32).clamp(1, 128) as usize;
    let mut budget = limit;
    let mut values = Vec::new();
    for root in roots.into_iter().skip(offset) {
        if let Some(value) = root.value(&mut budget, depth - 1) {
            values.push(value);
        }
        if budget == 0 {
            break;
        }
    }
    let next = offset.saturating_add(values.len());
    json!({"roots":values,"weight_unit":selected,"total_weight":total,"scope_self_weight":own,"path":path,"returned_nodes":limit-budget,"root_count":available,"next_cursor":if next<available{Some(next.to_string())}else{None},"node_limit":limit,"depth_limit":depth,"diagnostics":["Bounded tree: follow node IDs in path to inspect omitted children; cursor pages root siblings. Sampled weights are not exact call durations."]})
}

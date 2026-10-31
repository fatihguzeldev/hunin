use super::{function_identity, number};
use crate::query::WorkBudget;
use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Default)]
struct Tree {
    frame: Value,
    own: f64,
    total: f64,
    children: BTreeMap<String, Tree>,
    depth_truncated: bool,
}
impl Tree {
    fn add(
        &mut self,
        stack: &[Value],
        weight: f64,
        remaining: usize,
        work: &mut WorkBudget,
    ) -> Result<()> {
        work.check()?;
        self.total += weight;
        if remaining == 0 && !stack.is_empty() {
            self.depth_truncated = true;
            return Ok(());
        }
        if let Some((frame, rest)) = stack.split_first() {
            work.frame()?;
            let key = function_identity(frame);
            if !self.children.contains_key(&key) {
                work.entry()?;
            }
            self.children
                .entry(key)
                .or_insert_with(|| Tree {
                    frame: frame.clone(),
                    ..Default::default()
                })
                .add(rest, weight, remaining - 1, work)?;
        } else {
            self.own += weight;
        }
        Ok(())
    }
    fn value(
        self,
        budget: &mut usize,
        depth: usize,
        work: &mut WorkBudget,
    ) -> Result<Option<Value>> {
        work.check()?;
        if *budget == 0 {
            return Ok(None);
        }
        *budget -= 1;
        let id = frame_id(&self.frame);
        let count = self.children.len();
        let mut candidates = self.children.into_values().collect::<Vec<_>>();
        candidates.sort_by(|a, b| b.total.total_cmp(&a.total));
        work.check()?;
        let mut children = Vec::new();
        if depth > 0 {
            for child in candidates {
                if let Some(value) = child.value(budget, depth - 1, work)? {
                    children.push(value);
                }
                if *budget == 0 {
                    break;
                }
            }
        }
        Ok(Some(
            json!({"id":id,"name":self.frame["function"],"file":self.frame["file"],"line":self.frame["line"],"module":self.frame["module"],"code_id":self.frame["code_id"],"source_position_basis":"first observed sampled position","self_weight":self.own,"total_weight":self.total,"child_count":count,"children_truncated":children.len()<count||self.depth_truncated,"depth_truncated":self.depth_truncated,"children":children}),
        ))
    }
}
pub(super) fn frame_id(frame: &Value) -> String {
    format!(
        "frame:{:x}",
        Sha256::digest(function_identity(frame).as_bytes())
    )
}

pub fn tree(records: &[Value], requested_unit: &str) -> Value {
    tree_page(records, &json!({"weight_unit":requested_unit,"limit":5000}))
}

pub fn tree_page(records: &[Value], params: &Value) -> Value {
    let mut work = WorkBudget::unlimited();
    let mut page = TreeAccumulator::new(params);
    for record in records {
        page.add(record, &mut work).expect("unlimited query work");
    }
    page.finish(params, &mut work)
        .expect("unlimited query work")
}

pub(in crate::query) struct TreeAccumulator {
    selected: Option<String>,
    path: Vec<Value>,
    tree: Tree,
}
impl TreeAccumulator {
    pub(in crate::query) fn new(params: &Value) -> Self {
        let requested = params["weight_unit"].as_str().unwrap_or("");
        Self {
            selected: (!requested.is_empty()).then(|| requested.to_owned()),
            path: params["path"].as_array().cloned().unwrap_or_default(),
            tree: Tree::default(),
        }
    }
    pub(in crate::query) fn add(&mut self, record: &Value, work: &mut WorkBudget) -> Result<()> {
        work.check()?;
        if record["kind"] != "sample" {
            return Ok(());
        }
        let selected = self.selected.get_or_insert_with(|| {
            record["weight_unit"]
                .as_str()
                .unwrap_or("samples")
                .to_owned()
        });
        if record["weight_unit"] != *selected {
            return Ok(());
        }
        if let Some(stack) = record["stack"].as_array()
            && stack.len() >= self.path.len()
        {
            for (id, frame) in self.path.iter().zip(stack) {
                work.frame()?;
                if id.as_str() != Some(frame_id(frame).as_str()) {
                    return Ok(());
                }
            }
            self.tree.add(
                &stack[self.path.len()..],
                number(record, "weight"),
                128,
                work,
            )?;
        }
        Ok(())
    }
    pub(in crate::query) fn finish(self, params: &Value, work: &mut WorkBudget) -> Result<Value> {
        work.check()?;
        let total = self.tree.total;
        let own = self.tree.own;
        let mut roots = self.tree.children.into_values().collect::<Vec<_>>();
        roots.sort_by(|a, b| b.total.total_cmp(&a.total));
        work.check()?;
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
            if let Some(value) = root.value(&mut budget, depth - 1, work)? {
                values.push(value);
            }
            if budget == 0 {
                break;
            }
        }
        let next = offset.saturating_add(values.len());
        Ok(
            json!({"roots":values,"weight_unit":self.selected.unwrap_or_else(||"samples".into()),"total_weight":total,"scope_self_weight":own,"path":self.path,"returned_nodes":limit-budget,"root_count":available,"next_cursor":if next<available{Some(next.to_string())}else{None},"node_limit":limit,"depth_limit":depth,"diagnostics":["Bounded tree: follow node IDs in path to inspect omitted children; cursor pages root siblings. Sampled weights are not exact call durations."]}),
        )
    }
}

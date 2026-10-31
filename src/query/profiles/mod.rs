//! CPU stack views share measured frame identity; ranking and traversal differ.
mod hotspots;
mod inspection;
mod tree;

pub(super) use hotspots::HotspotAccumulator;
pub(crate) use hotspots::comparison_hotspots_checked;
pub use hotspots::hotspots;
pub(super) use inspection::inspect;
pub(super) use tree::TreeAccumulator;
pub use tree::{tree, tree_page};

use serde_json::{Value, json};

fn number(v: &Value, field: &str) -> f64 {
    v[field].as_f64().unwrap_or(0.0)
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

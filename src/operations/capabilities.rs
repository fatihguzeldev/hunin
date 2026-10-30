//! Compose collector prerequisite evidence and the independent offline import catalog.
use serde_json::{Value, json};

pub fn discover() -> Value {
    let mut capabilities = crate::collection::discovery::discover();
    capabilities["import_formats"] = json!(crate::importers::import_formats());
    capabilities
}

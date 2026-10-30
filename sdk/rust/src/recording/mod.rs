//! Explicit context/span lifetimes and idiomatic observation construction.
mod writer;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::time::Instant;
use writer::{emit, epoch_us, id, owns_state};
pub use writer::{enabled, flush, last_error};

fn object(value: Value) -> Value {
    if value.is_object() {
        value
    } else {
        json!({})
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Context {
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}
impl Context {
    pub fn current_run() -> Self {
        Self {
            run_id: writer::run_id(),
            ..Self::default()
        }
    }
    pub fn new(name: &str) -> Self {
        let mut context = Self::current_run();
        context.context_id = emit(&context, "context", json!({"name":name}));
        context
    }
    pub fn span(&self, name: &str) -> Span {
        Span::in_context(self, name)
    }
}
pub struct Span {
    pub id: Option<String>,
    context: Context,
    name: String,
    started: Instant,
    timestamp_us: u64,
    attributes: Map<String, Value>,
    ended: bool,
}
impl Span {
    pub fn new(name: &str) -> Self {
        Self::in_context(&Context::current_run(), name)
    }
    pub fn in_context(context: &Context, name: &str) -> Self {
        let active = enabled();
        let run = Context::current_run();
        Self {
            id: if active { id() } else { None },
            context: if context.run_id == run.run_id {
                context.clone()
            } else {
                run
            },
            name: name.into(),
            started: Instant::now(),
            timestamp_us: epoch_us(),
            attributes: Map::new(),
            ended: false,
        }
    }
    pub fn context(&self) -> Context {
        if !owns_state() {
            return Context::default();
        }
        Context {
            parent_id: self.id.clone(),
            ..self.context.clone()
        }
    }
    pub fn attribute(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.attributes.insert(key.into(), value.into());
    }
    pub fn end(mut self) {
        self.finish();
    }
    fn finish(&mut self) {
        if self.ended || !owns_state() {
            return;
        }
        self.ended = true;
        if let Some(id) = &self.id {
            let mut fields = json!({"id":id,"name":self.name,"timestamp_us":self.timestamp_us,"duration_us":self.started.elapsed().as_micros().min(9_007_199_254_740_991)as u64,"attributes":self.attributes});
            if let Some(parent) = &self.context.parent_id {
                fields["parent_id"] = json!(parent);
            }
            emit(&self.context, "span", fields);
        }
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        self.finish();
    }
}
pub fn counter(context: &Context, name: &str, value: f64, unit: &str) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    emit(
        context,
        "metric",
        json!({"name":name,"value":value,"unit":unit,"scope":if context.context_id.is_some(){"context"}else{"thread"},"attributes":{"temporality":"delta"}}),
    )
}
pub fn event(context: &Context, name: &str, attributes: Value) -> Option<String> {
    emit(
        context,
        "event",
        json!({"name":name,"attributes":object(attributes)}),
    )
}
pub fn relation(
    context: &Context,
    from: &str,
    to: &str,
    name: &str,
    evidence: &[String],
) -> Option<String> {
    let unique: std::collections::BTreeSet<_> = evidence.iter().collect();
    emit(
        context,
        "relation",
        json!({"from":from,"to":to,"relation":name,"evidence":unique}),
    )
}

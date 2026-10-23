//! Explicit contexts are safe to carry between threads and asynchronous tasks.
//! Observations are disabled unless HUNIN_RUN_ID and HUNIN_RECORD_DIR are set.
mod recording;
pub use recording::{counter, enabled, event, flush, last_error, relation, Context, Span};
pub use serde_json::{json, Value};

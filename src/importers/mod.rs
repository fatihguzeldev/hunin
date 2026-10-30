//! Offline profile normalization and retained source/image evidence.
mod firefox;
mod formats;
mod go_trace;
mod graal;
mod inputs;
mod javascript_sources;
mod jsonl;
mod output;
mod pipeline;
mod pprof;
mod python_cpu;
mod result;
mod v8;
mod vmprof;

pub(crate) use formats::import_formats;
pub(crate) use inputs::{Limits as NormalizationInputLimits, NormalizationByteLimit};
pub(crate) use output::{NormalizationOutputLimit, OutputBudget};
pub use pipeline::{import, import_with_native_context};
pub(crate) use pipeline::{
    import_controlled, import_with_budget_controlled, import_with_native_context_controlled,
};
pub use result::{BinarySnapshot, ImportedProfile, SourceSnapshot};

#[cfg(test)]
mod tests;

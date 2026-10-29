use serde_json::Value;

/// Canonical weighted-sample units produced by normalizers. This is an output
/// domain type, not a closed decoder for retained or extensible wire records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SampleWeightUnit {
    Samples,
    Microseconds,
    Bytes,
    Count,
}

impl SampleWeightUnit {
    pub(crate) fn as_wire(self) -> &'static str {
        match self {
            Self::Samples => "samples",
            Self::Microseconds => "microseconds",
            Self::Bytes => "bytes",
            Self::Count => "count",
        }
    }
}

/// Common normalized weighted stack measurement data. Input/result identity is assigned by
/// the acceptance codec; format-specific frame and attribution metadata remains
/// extensible while consumers migrate to the internal model.
#[derive(Debug)]
pub(crate) struct StackSample<'a> {
    pub pid: u64,
    pub tid: &'a str,
    pub timestamp_us: u64,
    pub stack: Vec<Value>,
    pub weight: f64,
    pub weight_unit: SampleWeightUnit,
    pub attributes: Value,
}

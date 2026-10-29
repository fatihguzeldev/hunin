use serde_json::Value;

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
    pub weight_unit: &'a str,
    pub attributes: Value,
}

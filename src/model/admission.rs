//! A pre-admission rejection from an actual scoped ownership counter.
use std::fmt;

#[derive(Debug)]
pub struct CapacityExceeded {
    pub scope: &'static str,
    pub maximum: usize,
}
impl CapacityExceeded {
    pub const fn new(scope: &'static str, maximum: usize) -> Self {
        Self { scope, maximum }
    }
}
impl fmt::Display for CapacityExceeded {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            output,
            "capacity reached for {}: at most {} owned units may be in flight",
            self.scope, self.maximum
        )
    }
}
impl std::error::Error for CapacityExceeded {}

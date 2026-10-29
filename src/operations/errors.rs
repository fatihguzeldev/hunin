//! Typed application failures and their additive wire projection.
use serde_json::{Value, json};
use std::fmt;

#[derive(Debug, Clone, Copy)]
enum Kind {
    UnknownOperation,
    InvalidParameters,
    Conflict,
    Capacity { scope: &'static str, maximum: usize },
    ReadAbandoned,
    ReadDeadline,
}

#[derive(Debug)]
pub(super) struct Failure {
    kind: Kind,
    message: String,
}

impl Failure {
    pub fn capacity(scope: &'static str, maximum: usize) -> Self {
        Self {
            kind: Kind::Capacity { scope, maximum },
            message: format!(
                "operation capacity reached for {scope}: at most {maximum} blocking units in flight; retry after current work settles"
            ),
        }
    }
    pub fn read_abandoned() -> Self {
        Self {
            kind: Kind::ReadAbandoned,
            message: "read request abandoned at a cooperative checkpoint".into(),
        }
    }
    pub fn read_deadline() -> Self {
        Self {
            kind: Kind::ReadDeadline,
            message: "read request exceeded its 10 second checkpoint budget".into(),
        }
    }
    pub fn unknown_operation() -> Self {
        Self {
            kind: Kind::UnknownOperation,
            message: "unknown operation".into(),
        }
    }
    pub fn invalid_parameters(message: impl fmt::Display) -> Self {
        Self {
            kind: Kind::InvalidParameters,
            message: message.to_string(),
        }
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            kind: Kind::Conflict,
            message: message.into(),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(&self.message)
    }
}
impl std::error::Error for Failure {}

/// The protocol schema owns the wire shape. This codec maps known typed origins
/// and leaves every other error generic; messages are never a classifier.
pub(crate) fn operation_failure(operation: &str, error: &anyhow::Error) -> Value {
    let code = match error.downcast_ref::<Failure>().map(|failure| failure.kind) {
        Some(Kind::UnknownOperation) => "unknown_operation",
        Some(Kind::InvalidParameters) => "invalid_parameters",
        Some(Kind::Conflict) => "conflict",
        Some(Kind::Capacity { .. }) => "capacity_exceeded",
        Some(Kind::ReadAbandoned) => "request_abandoned",
        Some(Kind::ReadDeadline) => "request_deadline_exceeded",
        None => "operation_failed",
    };
    let mut result = json!({"code":code,"operation":operation,"message":format!("{error:#}")});
    if let Some(Failure {
        kind: Kind::Capacity { scope, maximum },
        ..
    }) = error.downcast_ref::<Failure>()
    {
        result["capacity"] = json!({"scope":scope,"maximum_in_flight":maximum});
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn typed_origin_codecs_match_the_protocol_authority() {
        let errors = [
            anyhow::Error::new(Failure::unknown_operation()),
            anyhow::Error::new(Failure::invalid_parameters("invalid")),
            anyhow::Error::new(Failure::conflict("busy")),
            anyhow::Error::new(Failure::capacity("test_family", 2)),
            anyhow::Error::new(Failure::read_abandoned()),
            anyhow::Error::new(Failure::read_deadline()),
            anyhow::anyhow!("unknown operation invalid parameters conflict"),
        ];
        let actual: BTreeSet<_> = errors
            .iter()
            .map(|error| {
                operation_failure("control", error)["code"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let schema: Value =
            serde_json::from_str(include_str!("../../protocol/response.schema.json")).unwrap();
        let expected: BTreeSet<_> =
            schema["$defs"]["OperationFailure"]["properties"]["code"]["enum"]
                .as_array()
                .unwrap()
                .iter()
                .map(|code| code.as_str().unwrap().to_owned())
                .collect();
        assert_eq!(actual, expected);
    }
}

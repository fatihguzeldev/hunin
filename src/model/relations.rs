use super::identity::{ObservationId, ResultId};
/// The kind distinction needed by ownership rules. Other wire observation kinds
/// remain valid; the wire schema, rather than this projection, owns their list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnershipKind {
    Span,
    Context,
    Other,
}

impl OwnershipKind {
    pub fn from_wire(kind: &str) -> Self {
        match kind {
            "span" => Self::Span,
            "context" => Self::Context,
            _ => Self::Other,
        }
    }
}

/// Only relationship data participates in acceptance. Stack frames, source text
/// and arbitrary observation attributes are not part of the ownership graph.
#[derive(Debug, Clone)]
pub(crate) struct Ownership {
    pub kind: OwnershipKind,
    pub parent_id: Option<ObservationId>,
    pub context_id: Option<ObservationId>,
}

pub(crate) struct ObservationLinks {
    pub id: ObservationId,
    pub ownership: Ownership,
    pub references: Vec<ObservationId>,
}

pub(crate) struct SelectedObservation {
    pub ownership: Ownership,
    pub result_set_id: ResultId,
}

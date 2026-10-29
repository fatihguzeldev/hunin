//! Result acceptance policy. Persistence supplies a selected snapshot while
//! holding its write transaction; this module does not open storage or commit.

use crate::model::identity::{ObservationId, ResultId};
use crate::model::relations::{ObservationLinks, Ownership, OwnershipKind, SelectedObservation};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub(crate) struct PublicationRequest<'a> {
    pub run_id: &'a str,
    pub source_key: &'a str,
    pub input_identity: &'a str,
    pub records: &'a [Value],
    pub metadata: Option<PublicationMetadata<'a>>,
    pub deadline: Option<std::time::Instant>,
}

/// A checkpoint cannot preempt a synchronous unit or an in-flight disk commit.
/// Recheck after acquiring the writer and before beginning the commit.
pub(crate) fn check_deadline(deadline: Option<std::time::Instant>) -> Result<()> {
    if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        bail!("result publication deadline exceeded");
    }
    Ok(())
}

/// Metadata has a lifecycle role before it has a persistence namespace. Values
/// retain their existing extensible wire representation during the migration.
#[derive(Clone, Copy)]
pub(crate) enum PublicationMetadata<'a> {
    Analysis {
        execution_id: &'a str,
        document: &'a Value,
    },
    Normalization {
        input_id: &'a str,
        document: &'a Value,
    },
    Other {
        namespace: &'a str,
        id: &'a str,
        document: &'a Value,
    },
}

impl PublicationMetadata<'_> {
    pub fn analysis_execution_id(&self) -> Option<&str> {
        match self {
            Self::Analysis { execution_id, .. } => Some(execution_id),
            _ => None,
        }
    }

    pub fn document(&self) -> &Value {
        match self {
            Self::Analysis { document, .. }
            | Self::Normalization { document, .. }
            | Self::Other { document, .. } => document,
        }
    }

    pub fn declared_inputs(&self) -> Result<Vec<ResultId>> {
        let Some(inputs) = self.document().get("input_result_sets") else {
            return Ok(Vec::new());
        };
        inputs
            .as_array()
            .context("input_result_sets must be an array")?
            .iter()
            .map(|id| {
                id.as_str()
                    .context("input result identity must be a string")
                    .map(ResultId::from)
            })
            .collect()
    }

    pub fn replayed_document(&self, result_id: &ResultId) -> Value {
        let mut document = self.document().clone();
        document["status"] = json!("replayed");
        document["result_set_id"] = json!(result_id);
        document
    }
}

pub(crate) fn validate_relations(
    proposed: &[ObservationLinks],
    selected: &HashMap<ObservationId, SelectedObservation>,
) -> Result<HashSet<ResultId>> {
    validate_ownership(proposed, selected)?;
    dependencies(proposed, selected)
}

pub(crate) fn validate_ownership(
    proposed: &[ObservationLinks],
    selected: &HashMap<ObservationId, SelectedObservation>,
) -> Result<()> {
    let own: HashMap<&ObservationId, &Ownership> = proposed
        .iter()
        .map(|row| (&row.id, &row.ownership))
        .collect();
    let lookup = |id: &ObservationId| {
        own.get(id)
            .copied()
            .or_else(|| selected.get(id).map(|row| &row.ownership))
    };
    for row in proposed {
        for (field, id, kind, name) in [
            (
                "parent_id",
                row.ownership.parent_id.as_ref(),
                OwnershipKind::Span,
                "span",
            ),
            (
                "context_id",
                row.ownership.context_id.as_ref(),
                OwnershipKind::Context,
                "context",
            ),
        ] {
            if let Some(id) = id
                && lookup(id).is_none_or(|parent| parent.kind != kind)
            {
                bail!("{field} must reference a {name}: {id}");
            }
        }
        for (field, target) in [
            (
                "parent_id",
                parent_id as fn(&Ownership) -> Option<&ObservationId>,
            ),
            (
                "context_id",
                context_id as fn(&Ownership) -> Option<&ObservationId>,
            ),
        ] {
            let mut seen = HashSet::new();
            let mut current = Some((&row.id, &row.ownership));
            while let Some((id, ownership)) = current {
                if !seen.insert(id) {
                    bail!("cyclic {field} ownership");
                }
                current = target(ownership).and_then(|id| lookup(id).map(|node| (id, node)));
            }
        }
    }
    Ok(())
}

fn parent_id(ownership: &Ownership) -> Option<&ObservationId> {
    ownership.parent_id.as_ref()
}
fn context_id(ownership: &Ownership) -> Option<&ObservationId> {
    ownership.context_id.as_ref()
}

fn dependencies(
    proposed: &[ObservationLinks],
    selected: &HashMap<ObservationId, SelectedObservation>,
) -> Result<HashSet<ResultId>> {
    let own: HashSet<&ObservationId> = proposed.iter().map(|row| &row.id).collect();
    let mut dependencies = HashSet::new();
    for row in proposed {
        for reference in &row.references {
            if own.contains(reference) {
                continue;
            }
            let accepted = selected
                .get(reference)
                .ok_or_else(|| anyhow::anyhow!("unresolved observation reference: {reference}"))?;
            dependencies.insert(accepted.result_set_id.clone());
        }
    }
    Ok(dependencies)
}

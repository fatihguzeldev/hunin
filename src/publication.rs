//! Result acceptance policy. Persistence supplies a selected snapshot while
//! holding its write transaction; this module does not open storage or commit.

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

    pub fn declared_inputs(&self) -> Result<Vec<&str>> {
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
            })
            .collect()
    }

    pub fn replayed_document(&self, result_id: &str) -> Value {
        let mut document = self.document().clone();
        document["status"] = json!("replayed");
        document["result_set_id"] = json!(result_id);
        document
    }
}

pub(crate) fn validate_relations(
    proposed: &[ObservationLinks],
    selected: &HashMap<String, SelectedObservation>,
) -> Result<HashSet<String>> {
    validate_ownership(proposed, selected)?;
    dependencies(proposed, selected)
}

pub(crate) fn validate_ownership(
    proposed: &[ObservationLinks],
    selected: &HashMap<String, SelectedObservation>,
) -> Result<()> {
    let own: HashMap<&str, &Ownership> = proposed
        .iter()
        .map(|row| (row.id.as_str(), &row.ownership))
        .collect();
    let lookup = |id: &str| {
        own.get(id)
            .copied()
            .or_else(|| selected.get(id).map(|row| &row.ownership))
    };
    for row in proposed {
        for (field, id, kind, name) in [
            (
                "parent_id",
                row.ownership.parent_id.as_deref(),
                OwnershipKind::Span,
                "span",
            ),
            (
                "context_id",
                row.ownership.context_id.as_deref(),
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
            ("parent_id", parent_id as fn(&Ownership) -> Option<&str>),
            ("context_id", context_id as fn(&Ownership) -> Option<&str>),
        ] {
            let mut seen = HashSet::new();
            let mut current = Some((row.id.as_str(), &row.ownership));
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

fn parent_id(ownership: &Ownership) -> Option<&str> {
    ownership.parent_id.as_deref()
}
fn context_id(ownership: &Ownership) -> Option<&str> {
    ownership.context_id.as_deref()
}

fn dependencies(
    proposed: &[ObservationLinks],
    selected: &HashMap<String, SelectedObservation>,
) -> Result<HashSet<String>> {
    let own: HashSet<&str> = proposed.iter().map(|row| row.id.as_str()).collect();
    let mut dependencies = HashSet::new();
    for row in proposed {
        for reference in &row.references {
            if own.contains(reference.as_str()) {
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

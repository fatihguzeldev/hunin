//! Result acceptance policy. Persistence supplies a selected snapshot while
//! holding its write transaction; this module does not open storage or commit.

use crate::model::relations::{ObservationLinks, Ownership, OwnershipKind, SelectedObservation};
use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};

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

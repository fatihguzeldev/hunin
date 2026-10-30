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
            Self::Analysis { document, .. } | Self::Normalization { document, .. } => document,
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
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<HashSet<ResultId>> {
    validate_ownership_checked(proposed, selected, &mut checkpoint)?;
    dependencies(proposed, selected, checkpoint)
}

pub(crate) fn validate_ownership(
    proposed: &[ObservationLinks],
    selected: &HashMap<ObservationId, SelectedObservation>,
) -> Result<()> {
    validate_ownership_checked(proposed, selected, || Ok(()))
}

const MAXIMUM_OWNERSHIP_NODES: usize = 1_000_000;

fn validate_ownership_checked(
    proposed: &[ObservationLinks],
    selected: &HashMap<ObservationId, SelectedObservation>,
    checkpoint: impl FnMut() -> Result<()>,
) -> Result<()> {
    validate_ownership_bounded(proposed, selected, MAXIMUM_OWNERSHIP_NODES, checkpoint)
}

/// Each parent/context graph has one outgoing edge per node. Gray nodes detect
/// cycles; black nodes memoize complete paths, including shared selected
/// ancestors. An iterative walk avoids recursive stack growth. The named limit
/// counts distinct nodes across both graph walks, not bytes or wall time.
fn validate_ownership_bounded(
    proposed: &[ObservationLinks],
    selected: &HashMap<ObservationId, SelectedObservation>,
    maximum_nodes: usize,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<()> {
    let mut own = HashMap::new();
    for row in proposed {
        checkpoint()?;
        own.insert(&row.id, &row.ownership);
    }
    let lookup = |id: &ObservationId| {
        own.get(id)
            .copied()
            .or_else(|| selected.get(id).map(|row| &row.ownership))
    };
    let mut visited = 0usize;
    for (field, target, kind, name) in [
        (
            "parent_id",
            parent_id as fn(&Ownership) -> Option<&ObservationId>,
            OwnershipKind::Span,
            "span",
        ),
        (
            "context_id",
            context_id as fn(&Ownership) -> Option<&ObservationId>,
            OwnershipKind::Context,
            "context",
        ),
    ] {
        let mut colors: HashMap<&ObservationId, bool> = HashMap::new();
        for row in proposed {
            let mut path = Vec::new();
            let mut current = Some((&row.id, &row.ownership));
            while let Some((id, ownership)) = current {
                checkpoint()?;
                if let Some(active) = colors.get(id) {
                    if *active {
                        bail!("cyclic {field} ownership");
                    }
                    break;
                }
                visited += 1;
                anyhow::ensure!(
                    visited <= maximum_nodes,
                    "ownership-graph-v1 node limit exceeded: maximum {maximum_nodes}, observed {visited}"
                );
                colors.insert(id, true);
                path.push(id);
                current = if let Some(reference) = target(ownership) {
                    let parent = lookup(reference)
                        .filter(|parent| parent.kind == kind)
                        .with_context(|| format!("{field} must reference a {name}: {reference}"))?;
                    Some((reference, parent))
                } else {
                    None
                };
            }
            for id in path {
                checkpoint()?;
                colors.insert(id, false);
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
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<HashSet<ResultId>> {
    let own: HashSet<&ObservationId> = proposed.iter().map(|row| &row.id).collect();
    let mut dependencies = HashSet::new();
    for row in proposed {
        checkpoint()?;
        for reference in &row.references {
            checkpoint()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    fn chain(n: usize, context: bool) -> Vec<ObservationLinks> {
        (0..n)
            .map(|index| ObservationLinks {
                id: format!("node-{index}").into(),
                ownership: Ownership {
                    kind: if context {
                        OwnershipKind::Context
                    } else {
                        OwnershipKind::Span
                    },
                    parent_id: (!context && index > 0)
                        .then(|| format!("node-{}", index - 1).into()),
                    context_id: (context && index > 0)
                        .then(|| format!("node-{}", index - 1).into()),
                },
                references: vec![],
            })
            .collect()
    }
    #[test]
    fn long_parent_and_context_chains_have_linear_checkpoint_work() {
        for (context, reverse) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut proposed = chain(1000, context);
            if reverse {
                proposed.reverse();
            }
            let mut work = 0;
            validate_ownership_checked(&proposed, &HashMap::new(), || {
                work += 1;
                Ok(())
            })
            .unwrap();
            assert!(
                work <= 8 * proposed.len(),
                "{work} ownership steps for {} nodes",
                proposed.len()
            );
        }
    }
    #[test]
    fn shared_selected_ancestors_are_walked_once_and_keep_kind_and_cycle_checks() {
        let old = chain(400, false);
        let selected = old
            .into_iter()
            .map(|row| {
                (
                    row.id,
                    SelectedObservation {
                        ownership: row.ownership,
                        result_set_id: "selected".into(),
                    },
                )
            })
            .collect();
        let mut proposed = chain(400, false);
        for (index, row) in proposed.iter_mut().enumerate() {
            row.id = format!("leaf-{index}").into();
            row.ownership.parent_id = Some("node-399".into());
        }
        let mut work = 0;
        validate_ownership_checked(&proposed, &selected, || {
            work += 1;
            Ok(())
        })
        .unwrap();
        assert!(work <= 8 * 800, "{work} shared-ancestor steps");
        let mut cyclic = chain(2, false);
        cyclic[0].ownership.parent_id = Some("node-1".into());
        assert!(
            validate_ownership(&cyclic, &HashMap::new())
                .unwrap_err()
                .to_string()
                .contains("cyclic")
        );
        let mut wrong = chain(2, true);
        wrong[1].ownership.parent_id = Some("node-0".into());
        assert!(
            validate_ownership(&wrong, &HashMap::new())
                .unwrap_err()
                .to_string()
                .contains("span")
        );
    }
    #[test]
    fn cooperative_work_expiry_interrupts_ancestry_instead_of_finishing_the_chain() {
        let proposed = chain(1000, false);
        let mut work = 0;
        let error = validate_ownership_checked(&proposed, &HashMap::new(), || {
            work += 1;
            anyhow::ensure!(work < proposed.len() + 32, "injected traversal work limit");
            Ok(())
        })
        .unwrap_err();
        assert_eq!(work, proposed.len() + 32);
        assert!(error.to_string().contains("work limit"));
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn explicit_node_budget_rejects_distinct_work_even_with_a_live_deadline() {
        let records: Vec<_> = (0..100)
            .map(|i| ObservationLinks {
                id: format!("node-{i}").into(),
                ownership: Ownership {
                    kind: OwnershipKind::Span,
                    parent_id: None,
                    context_id: None,
                },
                references: vec![],
            })
            .collect();
        let error =
            validate_ownership_bounded(&records, &HashMap::new(), 20, || Ok(())).unwrap_err();
        assert!(
            error.to_string().contains("maximum 20, observed 21"),
            "{error}"
        );
    }
}

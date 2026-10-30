//! Project discovery is separate from each project's measurement store.
mod identity;
mod registry;
mod workspace;

pub use registry::ProjectRegistry;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Lightweight registry metadata; listing projects never opens their stores.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub store_path: PathBuf,
    pub workspace_roots: Vec<PathBuf>,
    /// The pre-project shared store is available without inferring membership.
    pub legacy: bool,
}

#[cfg(test)]
mod tests;

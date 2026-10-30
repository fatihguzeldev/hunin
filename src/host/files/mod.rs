//! Host file reads and directory inventory have separate work/lifetime bounds.
mod inventory;
mod reading;

pub(crate) use inventory::{InventoryLimits, inventory, walk_files};
pub(crate) use reading::{FileReadLimit, visit_regular};

//! Firefox processed profiles and their native image/lifetime evidence.
mod images;
mod input;
mod lifetimes;
mod profile;
mod sampling;

pub(super) use input::Input;
pub(super) const MAX_LIFETIME_SIDECAR: u64 = 160 * 1024 * 1024;

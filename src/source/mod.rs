//! Retained source custody and explicit measured/current-source read policy.
mod bundle;
mod paths;
mod reading;
mod retention;

pub(crate) use reading::{OutsideProject, read};
pub(crate) use retention::retain;

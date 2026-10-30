//! Retained source custody and explicit measured/current-source read policy.
mod paths;
mod reading;
mod retention;

pub(crate) use reading::read;
pub(crate) use retention::retain;

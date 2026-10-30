//! vmprof tool identity and sample normalization share a format boundary.
mod profile;
mod tool;
pub(super) use profile::decode_into;
pub(super) use tool::read;

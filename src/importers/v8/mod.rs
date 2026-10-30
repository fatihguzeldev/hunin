//! V8-compatible CPU profiles, runtime clock metadata and observed script/source maps.
mod input;
mod profile;
mod sources;
pub(super) use input::{metadata_paths, script_paths};
pub(super) use profile::decode_into;
pub(super) use sources::{MAX_TOTAL as MAX_SOURCE_BYTES, Scripts};

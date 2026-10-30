//! Registered analyzer packages: identity, admission and attempt materialization.
mod identity;
mod materialization;
mod registration;

pub(super) use identity::executable;
pub(super) use materialization::{copy_workspace, verify};
pub(super) use registration::register;

use super::files;

const MAX_FILES: usize = 2000;
const MAX_BYTES: u64 = 64 * 1024 * 1024;

fn source_read_error(error: anyhow::Error) -> anyhow::Error {
    if error.downcast_ref::<files::ByteLimitExceeded>().is_some() {
        error.context(
            "analyzer package exceeds the 64 MiB source limit; declare only required files",
        )
    } else {
        error
    }
}

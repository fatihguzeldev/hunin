//! Shared bounded prerequisite-command policy; never executes a measured workload.
use crate::host::probe as host_probe;
use std::{process::Command, time::Duration};

pub(super) const TIMEOUT_SECONDS: u64 = 2;
pub(super) const OUTPUT_BYTES: usize = 32 * 1024;

pub(super) fn checked(command: &mut Command) -> anyhow::Result<String> {
    let output = host_probe::output(
        command,
        None,
        Duration::from_secs(TIMEOUT_SECONDS),
        OUTPUT_BYTES,
    )?;
    if !output.status.success() {
        anyhow::bail!(
            "probe exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

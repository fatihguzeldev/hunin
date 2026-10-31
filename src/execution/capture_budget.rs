//! Check the owned capture inventory without truncating producer evidence.
use crate::{
    collection::{CaptureMode, CaptureRequest},
    host::{InventoryLimits, inventory},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

pub(super) const INTERVAL: Duration = Duration::from_millis(250);

pub(super) struct CaptureBudget {
    maximum: u64,
    observed: u64,
    failure: Option<String>,
}

impl CaptureBudget {
    pub fn for_request(request: &CaptureRequest) -> Option<Self> {
        (request.capture_mode == CaptureMode::CodeOrigin).then(|| Self {
            maximum: request
                .code_options
                .clone()
                .unwrap_or_default()
                .max_artifact_bytes,
            observed: 0,
            failure: None,
        })
    }
    pub fn observe(&mut self, result: Result<u64>) -> Option<String> {
        let failure = match result {
            Ok(bytes) => {
                self.observed = self.observed.max(bytes);
                (bytes > self.maximum).then(|| format!("code-origin capture artifact budget exceeded: observed {bytes} bytes, maximum {} bytes",self.maximum))
            }
            Err(error) => Some(format!(
                "code-origin capture artifact budget could not be checked: {error:#}"
            )),
        };
        self.failure = failure.clone();
        failure
    }
    pub fn metadata(&self) -> Value {
        json!({"status":if self.failure.is_some(){"stopped"}else{"checked"},"maximum_bytes":self.maximum,"maximum_observed_bytes":self.observed,"check_interval_us":INTERVAL.as_micros(),"scope":"all regular files in the owned capture directory, including stdout and stderr","policy":"metadata checks; producer writes can exceed the limit between checks; existing bytes are retained without truncation","failure":self.failure})
    }
}

pub(super) fn bytes(directory: &Path) -> Result<u64> {
    let deadline = Instant::now() + Duration::from_millis(100);
    let checkpoint = || {
        ensure!(
            Instant::now() < deadline,
            "capture budget inventory checkpoint elapsed"
        );
        Ok(())
    };
    let files = inventory(
        directory,
        &InventoryLimits {
            entries: 8192,
            files: 4096,
            depth: 8,
        },
        checkpoint,
    );
    ensure!(
        files.complete,
        "{}",
        files
            .failure
            .map(|failure| failure.diagnostic)
            .unwrap_or_else(|| "capture budget inventory incomplete".into())
    );
    let mut total = 0u64;
    for path in files.files {
        checkpoint()?;
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("capture budget metadata {}", path.display()))?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "capture budget entry changed type"
        );
        total = total
            .checked_add(metadata.len())
            .context("capture byte total overflow")?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_capture_budget_counts_nested_output_without_truncation() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        fs::write(root.path().join("stdout.txt"), b"stdout").unwrap();
        let evidence = root.path().join("nested/raw.log");
        fs::write(&evidence, b"raw evidence").unwrap();
        let mut budget = CaptureBudget {
            maximum: 10,
            observed: 0,
            failure: None,
        };
        assert!(
            budget
                .observe(bytes(root.path()))
                .unwrap()
                .contains("budget exceeded")
        );
        assert_eq!(budget.metadata()["maximum_observed_bytes"], 18);
        assert_eq!(fs::read(evidence).unwrap(), b"raw evidence");
    }

    #[cfg(unix)]
    #[test]
    fn capture_budget_does_not_follow_external_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("foreign"), vec![0u8; 100]).unwrap();
        std::os::unix::fs::symlink(external.path(), root.path().join("external")).unwrap();
        assert_eq!(bytes(root.path()).unwrap(), 0);
    }
}

use crate::Error;
use hunin_core::{project::TargetRef, records::SourceHandle};
use hunin_platforms::{ProcessInstance, ResourceSnapshot, process::ErrorKind, read_resources};
use serde_json::json;

pub(crate) struct ProcessResources {
    pub target: TargetRef,
    pub instance: ProcessInstance,
    pub source: SourceHandle,
    pub snapshots: usize,
    pub complete: bool,
    pub final_instance_verified: bool,
    first: ResourceSnapshot,
    last: ResourceSnapshot,
    attempts: usize,
    live: bool,
}

impl ProcessResources {
    pub fn new(
        target: TargetRef,
        instance: ProcessInstance,
        source: SourceHandle,
        first: ResourceSnapshot,
    ) -> Self {
        Self {
            target,
            instance,
            source,
            snapshots: 1,
            complete: true,
            final_instance_verified: true,
            first: first.clone(),
            last: first,
            attempts: 1,
            live: true,
        }
    }
    pub fn sample(&mut self) -> Result<Option<String>, Error> {
        if !self.live {
            return Ok(None);
        }
        self.attempts += 1;
        match read_resources(&self.instance) {
            Ok(snapshot) => {
                self.source
                    .append(&[snapshot.to_record(&self.snapshots.to_string(), &self.target.id)?])?;
                self.snapshots += 1;
                self.last = snapshot;
                self.final_instance_verified = true;
                Ok(None)
            }
            Err(error) => {
                self.complete = false;
                self.final_instance_verified = false;
                if matches!(
                    error.kind,
                    ErrorKind::ProcessGone | ErrorKind::InstanceChanged
                ) {
                    self.live = false;
                }
                self.source.report_gap(&format!("read-{}",self.attempts),json!({"error":error,"attempt":self.attempts,"meaning":"unavailable acquisition, not measured zero"}))?;
                Ok(Some(format!(
                    "resource process {}: {error}",
                    self.instance.pid
                )))
            }
        }
    }
    pub fn cpu_ns(&self) -> Result<u64, Error> {
        let sum = |snapshot: &ResourceSnapshot| -> Result<u64, Error> {
            snapshot
                .user_cpu
                .nanoseconds()?
                .checked_add(snapshot.system_cpu.nanoseconds()?)
                .ok_or(Error::Invalid("cpu sum overflow"))
        };
        sum(&self.last)?
            .checked_sub(sum(&self.first)?)
            .ok_or(Error::Invalid("process cpu counter moved backwards"))
    }
    pub fn close(&self) -> Result<(), Error> {
        if !self.source.status()?.closed {
            self.source.close(json!({"complete":self.complete,"coverage":"initial readiness through last successful held-process acquisition; application startup/final exit excluded","series_count":self.snapshots,"instance":self.instance,"scope":"process all threads excluding children"}))?;
        }
        Ok(())
    }
}

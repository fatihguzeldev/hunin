//! Recovery policy for interrupted recordings; OS owner evidence lives in host.
use crate::{
    host::process::owner::Probe,
    model::{now_us, runs::RunStatus},
    storage::Store,
};
use anyhow::Result;
use serde_json::json;

pub(super) fn recover_abandoned(store: &Store) -> Result<()> {
    let mut offset = 0;
    let mut running = Vec::new();
    loop {
        let runs = store.runs(250, offset)?;
        let count = runs.len();
        running.extend(
            runs.into_iter()
                .filter(|run| run.status == RunStatus::Running),
        );
        if count < 250 {
            break;
        }
        offset += count;
    }
    if running.is_empty() {
        return Ok(());
    }
    let owners = Probe::new(running.iter().map(|run| &run.source["owner"]));
    for mut run in running {
        if !owners.is_proven_absent(&run.source["owner"]) {
            continue;
        }
        run.status = RunStatus::Failed;
        run.finished_at_us = Some(now_us());
        run.diagnostics.push("Recording interrupted: its owning Hunin process is absent or has a different OS start identity. Preserved artifacts remain available; no stale PID was signaled.".into());
        run.source["collection_status"] = json!("interrupted");
        store.save_run(&run)?;
    }
    Ok(())
}

//! A runas ETW helper is outside the profiler's Job Object. Its retained
//! controller report acknowledges cleanup after the owned wrapper disappears.
use serde_json::{json, Value};
use std::{io::Read, path::Path, time::Duration};

pub(super) async fn wait(directory: &Path, timeout: Duration) -> Value {
    let path = directory.join("cpu.samply.xperf-session.json");
    let deadline = tokio::time::Instant::now() + timeout;
    let mut failed_report = None;
    loop {
        if let Ok(file) = std::fs::File::open(&path) {
            let mut bytes = Vec::new();
            if file.take(64 * 1024 + 1).read_to_end(&mut bytes).is_ok() && bytes.len() <= 64 * 1024
            {
                if let Ok(report) = serde_json::from_slice::<Value>(&bytes) {
                    if report["schema"] == "hunin-xperf-session-v1" {
                        let state = report["state"].as_str().unwrap_or("unknown");
                        if matches!(state, "stopped" | "refused") {
                            return json!({"acknowledged":true,"state":state,"reason":"helper confirmed owned sessions stopped or refused before starting","report":report,"scope":"owned ETW controllers; separate from target process exit"});
                        }
                        if state == "cleanup_failed" {
                            // Drop retries failed controllers once. Allow its
                            // bounded retry to replace this intermediate report.
                            failed_report = Some(report);
                        }
                    }
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            if let Some(report) = failed_report {
                return json!({"acknowledged":false,"state":"cleanup_failed","reason":"ETW controller reported cleanup failure without a later successful acknowledgement","report":report});
            }
            return json!({"acknowledged":false,"state":"unknown","reason":"bounded wait expired without final helper cleanup evidence","report_path":path,"scope":"no claim that an unobserved ETW session stopped"});
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cleanup_wait_observes_late_final_ack_and_bounds_missing_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cpu.samply.xperf-session.json");
        std::fs::write(
            &path,
            br#"{"schema":"hunin-xperf-session-v1","state":"cleanup_failed"}"#,
        )
        .unwrap();
        let writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            std::fs::write(path, br#"{"schema":"hunin-xperf-session-v1","state":"stopped","sessions":[{"control_handle":"0x1"}]}"#).unwrap();
        });
        let observed = wait(directory.path(), Duration::from_secs(1)).await;
        writer.await.unwrap();
        assert_eq!(observed["acknowledged"], true);
        assert_eq!(observed["report"]["sessions"][0]["control_handle"], "0x1");
        let absent = tempfile::tempdir().unwrap();
        let observed = wait(absent.path(), Duration::from_millis(20)).await;
        assert_eq!(observed["acknowledged"], false);
        assert_eq!(observed["state"], "unknown");
    }
}

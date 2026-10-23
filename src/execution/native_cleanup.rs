//! An elevated helper can inherit the profiler's Job when already elevated.
//! Request its owned-controller cleanup before forcing that Job; also await
//! a report bound to this exact wrapper birth and capture output path.
use serde_json::{Value, json};
use std::{io::Read, path::Path, time::Duration};

pub(super) async fn request_stop(directory: &Path, runtime: &str, pid: u32, birth: Option<&str>) {
    if !matches!(
        runtime.to_ascii_lowercase().as_str(),
        "go" | "rust" | "c" | "cpp" | "c++" | "native"
    ) {
        return;
    }
    let path = directory.join("cpu.samply.stop-request");
    if let Err(error) = std::fs::write(&path, b"stop owned native collection") {
        eprintln!("Could not request native session cleanup: {error}");
        return;
    }
    // Completion is separately retained by the final wait. Never force the Job
    // before giving its elevated session owner this bounded acknowledgement.
    let _ = wait(directory, pid, birth, Duration::from_secs(8)).await;
}

pub(super) async fn wait(
    directory: &Path,
    pid: u32,
    birth: Option<&str>,
    timeout: Duration,
) -> Value {
    let path = directory.join("cpu.samply.xperf-session.json");
    let deadline = tokio::time::Instant::now() + timeout;
    let mut failed_report = None;
    loop {
        if let Ok(file) = std::fs::File::open(&path) {
            let mut bytes = Vec::new();
            if file.take(64 * 1024 + 1).read_to_end(&mut bytes).is_ok() && bytes.len() <= 64 * 1024
            {
                if let Ok(report) = serde_json::from_slice::<Value>(&bytes) {
                    if report["schema"] == "hunin-xperf-session-v1"
                        && bound(&report, directory, pid, birth)
                    {
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

fn bound(report: &Value, directory: &Path, pid: u32, birth: Option<&str>) -> bool {
    let creation = birth
        .and_then(|birth| birth.strip_prefix("windows-creation-100ns:"))
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|time| *time > 0);
    creation.is_some()
        && report["owner"]["wrapper_pid"].as_u64() == Some(u64::from(pid))
        && report["owner"]["wrapper_creation_100ns"].as_u64() == creation
        && report["owner"]["profile_output_path"]
            .as_str()
            .is_some_and(|path| Path::new(path) == directory.join("cpu.samply.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stop_request_is_delivered_before_wait_and_stale_report_cannot_ack() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cpu.samply.xperf-session.json");
        let profile = directory.path().join("cpu.samply.json");
        let report = json!({"schema":"hunin-xperf-session-v1","state":"stopped","owner":{"wrapper_pid":99,"wrapper_creation_100ns":123,"profile_output_path":profile}});
        std::fs::write(&path, report.to_string()).unwrap();
        assert_eq!(
            wait(
                directory.path(),
                7,
                Some("windows-creation-100ns:123"),
                Duration::from_millis(20)
            )
            .await["acknowledged"],
            false
        );
        let request = directory.path().join("cpu.samply.stop-request");
        let helper = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            while !request.exists() {
                assert!(tokio::time::Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let mut report = report;
            report["owner"]["wrapper_pid"] = json!(7);
            std::fs::write(path, report.to_string()).unwrap();
        });
        request_stop(
            directory.path(),
            "native",
            7,
            Some("windows-creation-100ns:123"),
        )
        .await;
        helper.await.unwrap();
        assert_eq!(
            wait(
                directory.path(),
                7,
                Some("windows-creation-100ns:123"),
                Duration::from_millis(20)
            )
            .await["acknowledged"],
            true
        );
    }

    #[tokio::test]
    async fn cleanup_wait_observes_late_final_ack_and_bounds_missing_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cpu.samply.xperf-session.json");
        std::fs::write(
            &path,
            json!({"schema":"hunin-xperf-session-v1","state":"cleanup_failed","owner":{"wrapper_pid":7,"wrapper_creation_100ns":123,"profile_output_path":directory.path().join("cpu.samply.json")}}).to_string(),
        )
        .unwrap();
        let profile_path = directory.path().join("cpu.samply.json");
        let writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            std::fs::write(path, json!({"schema":"hunin-xperf-session-v1","state":"stopped","sessions":[{"control_handle":"0x1"}],"owner":{"wrapper_pid":7,"wrapper_creation_100ns":123,"profile_output_path":profile_path}}).to_string()).unwrap();
        });
        let observed = wait(
            directory.path(),
            7,
            Some("windows-creation-100ns:123"),
            Duration::from_secs(1),
        )
        .await;
        writer.await.unwrap();
        assert_eq!(observed["acknowledged"], true);
        assert_eq!(observed["report"]["sessions"][0]["control_handle"], "0x1");
        let absent = tempfile::tempdir().unwrap();
        let observed = wait(
            absent.path(),
            7,
            Some("windows-creation-100ns:123"),
            Duration::from_millis(20),
        )
        .await;
        assert_eq!(observed["acknowledged"], false);
        assert_eq!(observed["state"], "unknown");
    }
}

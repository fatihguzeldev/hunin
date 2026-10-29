use super::super::finalization::{CaptureProducer, ProducerCoverage};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

pub(super) fn validate(
    files: &[PathBuf],
    run_id: &str,
    required: bool,
) -> Result<ProducerCoverage> {
    let mut coverage = ProducerCoverage::default();
    let reports = files
        .iter()
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with("deno-workers-") && name.ends_with(".jsonl")
            })
        })
        .collect::<Vec<_>>();
    ensure!(
        !required || reports.len() == 1,
        "Deno worker finalization requires exactly one lifecycle report; found {}",
        reports.len()
    );
    for path in reports {
        let records = fs::read_to_string(path)?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str::<Value>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| {
                format!("invalid Deno worker finalization report {}", path.display())
            })?;
        let planned = records
            .first()
            .context("empty Deno worker finalization report")?;
        ensure!(
            planned["schema_version"] == 1
                && planned["runtime"] == "deno"
                && planned["run_id"] == run_id
                && planned["status"] == "planned",
            "Deno worker finalization report does not match this recording"
        );
        let producer = planned["producer_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .context("Deno worker finalization report has no producer identity")?;
        ensure!(
            path.file_name().unwrap().to_string_lossy() == format!("deno-workers-{producer}.jsonl"),
            "Deno worker finalization report filename does not match its producer"
        );
        let finished = records.last().unwrap();
        ensure!(
            finished["event"] == "finished" && finished["status"] == "complete",
            "Deno worker profile finalization incomplete: {}",
            finished["diagnostics"]
        );
        ensure!(
            finished["diagnostics"]
                .as_array()
                .is_some_and(Vec::is_empty),
            "Deno worker profile finalization reported errors: {}",
            finished["diagnostics"]
        );
        let pid = finished["pid"]
            .as_u64()
            .filter(|pid| *pid > 0)
            .context("Deno worker finalization report has no process identity")?;
        for record in &records[1..] {
            ensure!(
                ["schema_version", "runtime", "run_id", "producer_id"]
                    .iter()
                    .all(|field| record[field] == planned[field])
                    && record["pid"].as_u64() == Some(pid),
                "Deno worker finalization report changed recording or process identity"
            );
        }
        let mut registered = BTreeSet::new();
        for record in records
            .iter()
            .filter(|record| record["event"] == "worker_registered")
        {
            let id = record["registration_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .context("Deno worker registration has no identity")?;
            ensure!(registered.insert(id), "duplicate Deno worker registration");
        }
        let workers = finished["workers"]
            .as_array()
            .context("missing finalized Deno workers")?;
        let profiles = finished["profiles"]
            .as_array()
            .context("missing finalized Deno worker profiles")?;
        ensure!(
            workers.len() == registered.len() && profiles.len() == registered.len(),
            "Deno worker finalization omitted registered workers"
        );
        let mut worker_ids = BTreeSet::new();
        let mut registrations_by_worker = BTreeMap::new();
        for worker in workers {
            let registration = worker["registration_id"]
                .as_str()
                .context("missing Deno worker registration identity")?;
            ensure!(
                registered.remove(registration),
                "Deno worker finalization substituted a worker"
            );
            let id = worker["worker_id"]
                .as_u64()
                .filter(|id| *id > 0 && *id <= u32::MAX as u64)
                .context("missing Deno runtime worker identity")?;
            ensure!(
                worker_ids.insert(id),
                "duplicate Deno runtime worker identity"
            );
            registrations_by_worker.insert(id, registration.to_owned());
            coverage.expected.push(registration.to_owned());
        }
        for profile in profiles {
            let id = profile["worker_id"]
                .as_u64()
                .context("missing Deno profile worker identity")?;
            ensure!(
                worker_ids.remove(&id),
                "Deno worker finalization substituted a profile"
            );
            let filename = profile["filename"]
                .as_str()
                .context("missing finalized Deno profile filename")?;
            let timestamp = filename
                .strip_prefix("CPU.")
                .and_then(|name| name.strip_suffix(&format!(".{pid}.worker-{id}.cpuprofile")));
            ensure!(
                timestamp
                    .is_some_and(|value| !value.is_empty()
                        && value.bytes().all(|byte| byte.is_ascii_digit())),
                "Deno profile filename does not match its process and worker"
            );
            let profile_path = path.parent().unwrap().join(filename);
            ensure!(
                files.contains(&profile_path),
                "finalized Deno worker profile is missing: {filename}"
            );
            let digest = format!("{:x}", Sha256::digest(fs::read(&profile_path)?));
            ensure!(
                profile["sha256"].as_str() == Some(digest.as_str()),
                "finalized Deno worker profile changed: {filename}"
            );
            coverage.observed.push(CaptureProducer {
                id: registrations_by_worker[&id].clone(),
                process_id: pid,
                runtime_id: id.to_string(),
                profiles: vec![profile_path],
            });
        }
    }
    coverage.expected.sort();
    coverage
        .observed
        .sort_by(|left, right| left.id.cmp(&right.id));
    Ok(coverage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn capture() -> (tempfile::TempDir, Vec<PathBuf>, Vec<Value>) {
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path().join("CPU.123456.123.worker-1.cpuprofile");
        fs::write(&profile, b"{}").unwrap();
        let identity = json!({"schema_version":1,"runtime":"deno","run_id":"recording","producer_id":"session"});
        let mut planned = identity.clone();
        planned["status"] = json!("planned");
        let mut registered = identity.clone();
        registered["pid"] = json!(123);
        registered["event"] = json!("worker_registered");
        registered["registration_id"] = json!("worker");
        registered["status"] = json!("pending");
        let mut finished = identity;
        finished["pid"] = json!(123);
        finished["event"] = json!("finished");
        finished["status"] = json!("complete");
        finished["diagnostics"] = json!([]);
        finished["workers"] = json!([{"registration_id":"worker","worker_id":1}]);
        finished["profiles"] = json!([{"worker_id":1,"filename":"CPU.123456.123.worker-1.cpuprofile",
            "sha256":"44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"}]);
        let report = directory.path().join("deno-workers-session.jsonl");
        (
            directory,
            vec![profile, report],
            vec![planned, registered, finished],
        )
    }

    fn write_report(files: &[PathBuf], records: &[Value]) {
        fs::write(
            &files[1],
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
    }

    #[test]
    fn missing_or_ambiguous_lifecycle_reports_cannot_pass() {
        let (_directory, files, records) = capture();
        write_report(&files, &records);
        assert!(validate(&files[..1], "recording", true).is_err());
        let mut duplicated = files.clone();
        duplicated.push(files[1].clone());
        assert!(validate(&duplicated, "recording", true).is_err());
    }

    #[test]
    fn requires_completed_worker_bytes_in_the_same_recording() {
        let (_directory, files, records) = capture();
        write_report(&files, &records);
        validate(&files, "recording", true).unwrap();
        assert!(validate(&files, "another-recording", true).is_err());
        fs::write(&files[0], b"{").unwrap();
        assert!(
            validate(&files, "recording", true)
                .unwrap_err()
                .to_string()
                .contains("profile changed")
        );
        fs::remove_file(&files[0]).unwrap();
        assert!(validate(&files, "recording", true).is_err());
    }

    #[test]
    fn forced_exit_or_explicit_incompletion_cannot_pass() {
        let (_directory, files, mut records) = capture();
        write_report(&files, &records[..2]);
        assert!(validate(&files, "recording", true).is_err());
        records[2]["status"] = json!("incomplete");
        records[2]["diagnostics"] = json!(["worker flush deadline expired"]);
        write_report(&files, &records);
        assert!(
            validate(&files, "recording", true)
                .unwrap_err()
                .to_string()
                .contains("worker flush deadline expired")
        );
        records[2]["status"] = json!("complete");
        write_report(&files, &records);
        assert!(validate(&files, "recording", true).is_err());
    }

    #[test]
    fn an_unrelated_profile_cannot_replace_a_missing_worker() {
        let (_directory, files, mut records) = capture();
        records[2]["workers"][0]["worker_id"] = json!(2);
        write_report(&files, &records);
        assert!(validate(&files, "recording", true).is_err());
        records[2]["workers"][0]["worker_id"] = json!(1);
        records[2]["profiles"] = json!([]);
        write_report(&files, &records);
        assert!(validate(&files, "recording", true).is_err());
    }

    #[test]
    fn changed_owner_or_profile_location_cannot_pass() {
        let (_directory, files, mut records) = capture();
        records[2]["pid"] = json!(456);
        write_report(&files, &records);
        assert!(validate(&files, "recording", true).is_err());
        records[2]["pid"] = json!(123);
        records[2]["producer_id"] = json!("another-session");
        write_report(&files, &records);
        assert!(validate(&files, "recording", true).is_err());
        records[2]["producer_id"] = json!("session");
        records[2]["profiles"][0]["filename"] = json!("../CPU.123456.123.worker-1.cpuprofile");
        write_report(&files, &records);
        assert!(validate(&files, "recording", true).is_err());
    }
}

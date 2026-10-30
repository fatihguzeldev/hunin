use super::super::{CaptureProducer, finalization::ProducerCoverage};
use super::ValidationReads;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
#[cfg(test)]
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[cfg(test)]
use std::fs;

#[derive(Deserialize)]
struct PlannedSession {
    schema_version: u64,
    runtime: String,
    run_id: String,
    session_id: String,
    root_registration_id: String,
    status: String,
}

#[derive(Deserialize)]
struct Producer {
    schema_version: u64,
    runtime: String,
    run_id: String,
    session_id: String,
    producer_id: String,
    root_registration_id: Option<String>,
    pid: u64,
    tid: u64,
    is_main_thread: bool,
    status: String,
    segments: Vec<Segment>,
    workers: Vec<Worker>,
    diagnostics: Vec<String>,
}

#[derive(Deserialize)]
struct Segment {
    sequence: u64,
    filename: String,
    sha256: String,
    clock_sha256: String,
}

#[derive(Deserialize)]
struct Worker {
    registration_id: String,
    thread_id: u64,
}

fn read_report<T: serde::de::DeserializeOwned>(
    path: &Path,
    reads: &mut ValidationReads,
) -> Result<T> {
    let bytes = reads.metadata(path)?;
    let result = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid Bun producer report {}", path.display()));
    reads.check()?;
    result
}

pub(super) fn validate_with_reads(
    files: &[PathBuf],
    run_id: &str,
    directory: &Path,
    session_id: &str,
    root_registration_id: &str,
    reads: &mut ValidationReads,
) -> Result<ProducerCoverage> {
    let planned_path = directory.join(format!("bun-session-{session_id}.json"));
    ensure!(
        files.contains(&planned_path),
        "planned Bun capture session is missing"
    );
    let planned: PlannedSession = read_report(&planned_path, reads)?;
    ensure!(
        planned.schema_version == 1
            && planned.runtime == "bun"
            && planned.run_id == run_id
            && planned.session_id == session_id
            && planned.root_registration_id == root_registration_id
            && planned.status == "planned",
        "Bun capture session does not match this recording"
    );
    let prefix = format!("bun-producer-{session_id}-");
    let mut producers = BTreeMap::new();
    let mut runtime_threads = BTreeMap::new();
    for path in files.iter().filter(|path| {
        path.parent() == Some(directory)
            && path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with(&prefix) && name.ends_with(".json")
            })
    }) {
        let producer: Producer = read_report(path, reads)?;
        ensure!(
            producer.schema_version == 1
                && producer.runtime == "bun"
                && producer.run_id == run_id
                && producer.session_id == session_id
                && !producer.producer_id.is_empty(),
            "Bun producer changed recording or session identity"
        );
        ensure!(
            path.file_name().unwrap().to_string_lossy()
                == format!("{prefix}{}.json", producer.producer_id),
            "Bun producer filename does not match its identity"
        );
        ensure!(
            producer.pid > 0
                && producer.pid <= u32::MAX as u64
                && producer.tid <= 9_007_199_254_740_991
                && producer.is_main_thread == (producer.tid == 0),
            "Bun producer has an invalid process or thread identity"
        );
        ensure!(
            producer.root_registration_id.is_none() || producer.is_main_thread,
            "Bun worker cannot claim a root registration"
        );
        ensure!(
            runtime_threads
                .insert((producer.pid, producer.tid), producer.producer_id.clone())
                .is_none(),
            "ambiguous Bun producer runtime thread identity"
        );
        ensure!(
            producers
                .insert(producer.producer_id.clone(), producer)
                .is_none(),
            "duplicate Bun producer identity"
        );
    }
    let roots = producers
        .values()
        .filter(|producer| producer.root_registration_id.as_deref() == Some(root_registration_id))
        .count();
    ensure!(
        roots == 1,
        "planned Bun root producer did not start exactly once; found {roots}"
    );
    let mut registrations = BTreeSet::new();
    let mut worker_claims = BTreeSet::new();
    let mut claimed_profiles = BTreeSet::new();
    let mut coverage = ProducerCoverage::default();
    for producer in producers.values() {
        ensure!(
            producer
                .root_registration_id
                .as_deref()
                .is_none_or(|id| id == root_registration_id),
            "Bun producer substituted a root registration"
        );
        ensure!(
            producer.status == "complete" && producer.diagnostics.is_empty(),
            "Bun producer {} finalization incomplete ({}): {}",
            producer.producer_id,
            producer.status,
            producer.diagnostics.join("; ")
        );
        ensure!(
            !producer.segments.is_empty(),
            "Bun producer {} has no finalized CPU segment",
            producer.producer_id
        );
        for worker in &producer.workers {
            reads.check()?;
            ensure!(
                worker.thread_id != producer.tid,
                "Bun producer cannot register its own thread as a child worker"
            );
            ensure!(
                !worker.registration_id.is_empty() && registrations.insert(&worker.registration_id),
                "duplicate or empty Bun worker registration"
            );
            ensure!(
                worker.thread_id > 0
                    && runtime_threads.contains_key(&(producer.pid, worker.thread_id)),
                "registered Bun worker {} in process {} did not start or finalize",
                worker.thread_id,
                producer.pid
            );
            ensure!(
                worker_claims.insert((producer.pid, worker.thread_id)),
                "Bun worker has multiple creator registrations"
            );
        }
        let mut paths = Vec::new();
        for (index, segment) in producer.segments.iter().enumerate() {
            reads.check()?;
            ensure!(
                segment.sequence == index as u64 + 1,
                "Bun producer CPU segment sequence has a gap"
            );
            let expected = format!(
                "CPU.{}.{}.{}.{}.cpuprofile",
                producer.pid, producer.tid, producer.producer_id, segment.sequence
            );
            ensure!(
                segment.filename == expected,
                "Bun CPU segment filename does not match its producer identity"
            );
            let profile = directory.join(&segment.filename);
            let clock = directory.join(format!("{}.meta.json", segment.filename));
            ensure!(
                files.contains(&profile) && files.contains(&clock),
                "finalized Bun CPU segment or clock metadata is missing: {expected}"
            );
            ensure!(
                claimed_profiles.insert(profile.clone()),
                "duplicate finalized Bun CPU segment"
            );
            ensure!(
                reads.raw_digest(&profile)? == segment.sha256
                    && reads.metadata_digest(&clock)? == segment.clock_sha256,
                "finalized Bun CPU segment or clock metadata changed: {expected}"
            );
            paths.push(profile);
        }
        coverage.expected.push(producer.producer_id.clone());
        coverage.observed.push(CaptureProducer {
            id: producer.producer_id.clone(),
            process_id: producer.pid,
            runtime_id: producer.tid.to_string(),
            profiles: paths,
        });
    }
    for path in files
        .iter()
        .filter(|path| super::super::artifacts::profile_format(path) == Some("v8-cpuprofile"))
    {
        ensure!(
            claimed_profiles.contains(path),
            "Bun capture produced an undeclared CPU profile: {}",
            path.display()
        );
    }
    reads.check()?;
    Ok(coverage)
}

#[cfg(test)]
fn validate(
    files: &[PathBuf],
    run_id: &str,
    directory: &Path,
    session_id: &str,
    root_registration_id: &str,
) -> Result<ProducerCoverage> {
    validate_with_reads(
        files,
        run_id,
        directory,
        session_id,
        root_registration_id,
        &mut ValidationReads::new(None),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn fixture() -> (tempfile::TempDir, Value, Value) {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("bun-session-session.json"), json!({
            "schema_version":1,"runtime":"bun","run_id":"recording","session_id":"session","root_registration_id":"root","status":"planned"
        }).to_string()).unwrap();
        let producer = |id: &str, tid, root, workers| {
            let filename = format!("CPU.123.{tid}.{id}.1.cpuprofile");
            fs::write(directory.path().join(&filename), b"raw profile").unwrap();
            fs::write(
                directory.path().join(format!("{filename}.meta.json")),
                b"raw clock",
            )
            .unwrap();
            json!({"schema_version":1,"runtime":"bun","run_id":"recording","session_id":"session","producer_id":id,
                "root_registration_id":root,"pid":123,"tid":tid,"is_main_thread":tid==0,"status":"complete","diagnostics":[],"workers":workers,
                "segments":[{"sequence":1,"filename":filename,"sha256":format!("{:x}",Sha256::digest(b"raw profile")),"clock_sha256":format!("{:x}",Sha256::digest(b"raw clock"))}]})
        };
        let main = producer(
            "main",
            0,
            Some("root"),
            json!([{"registration_id":"worker-registration","thread_id":1}]),
        );
        let worker = producer("worker", 1, None::<&str>, json!([]));
        (directory, main, worker)
    }

    fn write(directory: &Path, main: &Value, worker: Option<&Value>) -> Vec<PathBuf> {
        fs::write(
            directory.join("bun-producer-session-main.json"),
            main.to_string(),
        )
        .unwrap();
        if let Some(worker) = worker {
            fs::write(
                directory.join("bun-producer-session-worker.json"),
                worker.to_string(),
            )
            .unwrap();
        }
        let mut files = Vec::new();
        crate::host::walk_files(directory, &mut files).unwrap();
        files
    }

    #[test]
    fn validates_declared_root_workers_and_all_profile_clock_bytes() {
        let (directory, main, worker) = fixture();
        let files = write(directory.path(), &main, Some(&worker));
        let coverage = validate(&files, "recording", directory.path(), "session", "root").unwrap();
        assert_eq!(coverage.expected, ["main", "worker"]);
        assert_eq!(coverage.observed.len(), 2);
        assert_eq!(coverage.observed[1].runtime_id, "1");
        assert!(
            validate(
                &files,
                "another-recording",
                directory.path(),
                "session",
                "root"
            )
            .is_err()
        );
    }

    #[test]
    fn validation_limits_reject_before_accepting_complete_reports_and_preserve_raw() {
        let (directory, main, worker) = fixture();
        let files = write(directory.path(), &main, Some(&worker));
        let limits = super::super::validation::Limits {
            profile_bytes: 10,
            ..Default::default()
        };
        let mut reads = ValidationReads::with_limits(limits, None);
        let error = validate_with_reads(
            &files,
            "recording",
            directory.path(),
            "session",
            "root",
            &mut reads,
        )
        .unwrap_err();
        assert!(error.is::<crate::host::FileReadLimit>());
        let limits = super::super::validation::Limits {
            metadata_bytes: 1,
            ..Default::default()
        };
        let mut reads = ValidationReads::with_limits(limits, None);
        let error = validate_with_reads(
            &files,
            "recording",
            directory.path(),
            "session",
            "root",
            &mut reads,
        )
        .unwrap_err();
        assert!(error.is::<crate::host::FileReadLimit>());
        assert_eq!(
            fs::read(directory.path().join("CPU.123.0.main.1.cpuprofile")).unwrap(),
            b"raw profile"
        );
        assert!(
            directory
                .path()
                .join("bun-producer-session-worker.json")
                .is_file()
        );
    }

    #[test]
    fn main_and_earlier_worker_profiles_cannot_replace_worker_completion() {
        let (directory, main, mut worker) = fixture();
        let files = write(directory.path(), &main, None);
        assert!(
            validate(&files, "recording", directory.path(), "session", "root")
                .unwrap_err()
                .to_string()
                .contains("did not start or finalize")
        );
        worker["status"] = json!("incomplete");
        worker["diagnostics"] = json!(["final save failed"]);
        let files = write(directory.path(), &main, Some(&worker));
        assert!(
            validate(&files, "recording", directory.path(), "session", "root")
                .unwrap_err()
                .to_string()
                .contains("final save failed")
        );
    }

    #[test]
    fn changed_missing_or_undeclared_profile_bytes_cannot_pass() {
        let (directory, main, worker) = fixture();
        let files = write(directory.path(), &main, Some(&worker));
        let clock = directory
            .path()
            .join("CPU.123.1.worker.1.cpuprofile.meta.json");
        fs::write(&clock, b"changed").unwrap();
        assert!(validate(&files, "recording", directory.path(), "session", "root").is_err());
        fs::remove_file(clock).unwrap();
        let files = write(directory.path(), &main, Some(&worker));
        assert!(validate(&files, "recording", directory.path(), "session", "root").is_err());
        let (directory, main, worker) = fixture();
        fs::write(directory.path().join("unreported.cpuprofile"), b"{}").unwrap();
        let files = write(directory.path(), &main, Some(&worker));
        assert!(
            validate(&files, "recording", directory.path(), "session", "root")
                .unwrap_err()
                .to_string()
                .contains("undeclared CPU profile")
        );
    }

    #[test]
    fn missing_root_wrong_process_or_profile_substitution_cannot_pass() {
        let (directory, mut main, mut worker) = fixture();
        main["root_registration_id"] = Value::Null;
        let files = write(directory.path(), &main, Some(&worker));
        assert!(validate(&files, "recording", directory.path(), "session", "root").is_err());
        main["root_registration_id"] = json!("root");
        worker["pid"] = json!(456);
        let files = write(directory.path(), &main, Some(&worker));
        assert!(validate(&files, "recording", directory.path(), "session", "root").is_err());
        worker["pid"] = json!(123);
        worker["workers"] = json!([{"registration_id":"self-worker","thread_id":1}]);
        let files = write(directory.path(), &main, Some(&worker));
        assert!(
            validate(&files, "recording", directory.path(), "session", "root")
                .unwrap_err()
                .to_string()
                .contains("own thread")
        );
        worker["workers"] = json!([]);
        worker["segments"][0]["filename"] = json!("../outside.cpuprofile");
        let files = write(directory.path(), &main, Some(&worker));
        assert!(validate(&files, "recording", directory.path(), "session", "root").is_err());
    }
}

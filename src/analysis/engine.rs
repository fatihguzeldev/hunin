use super::{
    blocking, dependencies,
    input::{self, Input},
    package, process, retention,
};
use crate::{model::now_us, storage::Store};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};
use tokio::{
    sync::{Mutex, Semaphore, watch},
    time::Instant,
};

#[derive(Clone)]
pub struct Engine {
    pub(super) store: Store,
    pub(super) closing: Arc<AtomicBool>,
    pub(super) owner: crate::host::process::owner::Owner,
    pub(super) active: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    pub(super) slots: Arc<Semaphore>,
    pub(super) maximum_in_flight: usize,
}
impl Engine {
    pub fn new(store: Store) -> Self {
        Self::with_admission_limit(store, 2).expect("the default admission limit is valid")
    }
    /// Bound attempts shared by this Engine and its clones, from preparation
    /// through terminal retention/publication. Separate Engines are independent;
    /// this is neither a memory quota nor a lock across processes or a store.
    pub fn with_admission_limit(store: Store, maximum_in_flight: usize) -> Result<Self> {
        anyhow::ensure!(
            maximum_in_flight > 0 && maximum_in_flight <= Semaphore::MAX_PERMITS,
            "analysis admission limit must be positive and supported by the semaphore"
        );
        Ok(Self {
            store,
            closing: Arc::new(AtomicBool::new(false)),
            owner: crate::host::process::owner::Owner::current(),
            active: Arc::new(Mutex::new(HashMap::new())),
            slots: Arc::new(Semaphore::new(maximum_in_flight)),
            maximum_in_flight,
        })
    }
    pub fn register(&self, params: &Value) -> Result<Value> {
        package::register(&self.store, params)
    }
    async fn analyze(
        &self,
        analysis: &Arc<Value>,
        input: &Arc<Input>,
        directory: &Path,
        cancellation: watch::Receiver<bool>,
        deadline: Instant,
        timings: &mut Value,
    ) -> Result<Vec<Value>> {
        let control = process::Control::new(cancellation, deadline);
        control.check()?;
        let phase = Instant::now();
        let package = analysis.clone();
        let check = control.clone();
        let source = blocking(move || {
            check.check()?;
            let source = package::verify(&package, || check.check())?;
            check.check()?;
            Ok(source)
        })
        .await;
        timings["package_verification"] = json!(phase.elapsed().as_micros() as u64);
        let source = source?;
        control.check()?;
        let phase = Instant::now();
        let workspace = directory.join("workspace");
        let package = analysis.clone();
        let check = control.clone();
        let destination = workspace.clone();
        blocking(move || {
            package::copy_workspace(&package, &source, &destination, || check.check())
        })
        .await?;
        timings["workspace_copy"] = json!(phase.elapsed().as_micros() as u64);
        control.check()?;
        let phase = Instant::now();
        let prepared =
            dependencies::prepare(analysis, &workspace, directory, control.clone()).await;
        timings["dependency_setup"] = json!(phase.elapsed().as_micros() as u64);
        let (command, metadata) = prepared?;
        control.check()?;
        let destination = directory.join("dependencies.json");
        let check = control.clone();
        blocking(move || {
            check.check()?;
            fs::write(destination, serde_json::to_vec_pretty(&metadata)?)?;
            check.check()
        })
        .await?;
        let phase = Instant::now();
        let status = process::execute(
            &command,
            &workspace,
            Some(&directory.join("input.json")),
            &directory.join("stdout.jsonl"),
            &directory.join("stderr.txt"),
            control.clone(),
        )
        .await;
        timings["subprocess"] = json!(phase.elapsed().as_micros() as u64);
        let status = status?;
        control.check()?;
        if !status.success() {
            bail!(
                "analyzer exited unsuccessfully ({:?}); inspect its retained stdout/stderr",
                status.code()
            );
        }
        let phase = Instant::now();
        let package = analysis.clone();
        let selected = input.clone();
        let output = directory.join("stdout.jsonl");
        let check = control.clone();
        let records =
            blocking(move || validate_output(&package, &selected, &output, &check)).await?;
        timings["output_validation"] = json!(phase.elapsed().as_micros() as u64);
        control.check()?;
        let phase = Instant::now();
        let store = self.store.clone();
        let package = analysis.clone();
        let selected = input.clone();
        let check = control.clone();
        let current =
            blocking(move || input::snapshot(&store, &package, &selected.run_id, || check.check()))
                .await;
        timings["input_recheck"] = json!(phase.elapsed().as_micros() as u64);
        if current?.fingerprint != input.fingerprint {
            bail!("analysis inputs changed during execution; no stale output was published");
        }
        control.check()?;
        let phase = Instant::now();
        let package = analysis.clone();
        let check = control.clone();
        let verified = blocking(move || {
            check.check()?;
            let source = package::verify(&package, || check.check())?;
            check.check()?;
            Ok(source)
        })
        .await;
        timings["final_package_verification"] = json!(phase.elapsed().as_micros() as u64);
        verified?;
        control.check()?;
        Ok(records)
    }
    pub(super) async fn complete(
        &self,
        analysis: Arc<Value>,
        input: Arc<Input>,
        mut document: Value,
        directory: PathBuf,
        cancellation: watch::Receiver<bool>,
        deadline: Instant,
    ) {
        let mut timings = document["phase_timings_us"].clone();
        let result = self
            .analyze(
                &analysis,
                &input,
                &directory,
                cancellation.clone(),
                deadline,
                &mut timings,
            )
            .await;
        document["phase_timings_us"] = timings;
        let control = process::Control::new(cancellation, deadline);
        let engine = self.clone();
        let _ = blocking(move || {
            engine.finish(&analysis, &input, document, &directory, &control, result);
            Ok(())
        })
        .await;
    }

    fn finish(
        &self,
        analysis: &Value,
        input: &Input,
        mut document: Value,
        directory: &Path,
        control: &process::Control,
        result: Result<Vec<Value>>,
    ) {
        let id = document["id"].as_str().unwrap().to_owned();
        let requested = self
            .store
            .document("analysis-results", &id)
            .is_ok_and(|row| row["cancellation_requested"] == true);
        document["cancellation_requested"] = json!(requested);
        document["finished_at_us"] = json!(now_us());
        document["status"] = json!(if requested || *control.cancellation.borrow() {
            "cancelled"
        } else if result.is_ok() {
            "accepted"
        } else if Instant::now() >= control.deadline {
            "timed_out"
        } else {
            "failed"
        });
        if let Err(error) = &result {
            document["diagnostics"] = json!([format!("{error:#}")]);
        }
        let run_id = document["run_id"].as_str().unwrap().to_owned();
        let id = document["id"].as_str().unwrap().to_owned();
        let retention = Instant::now();
        let retained = retention::retain(
            &self.store,
            &mut document,
            directory,
            retention::Limits::default(),
            || control.check(),
        );
        document["phase_timings_us"]["retention"] = json!(retention.elapsed().as_micros() as u64);
        if let Err(error) = retained {
            let requested = self
                .store
                .document("analysis-results", &id)
                .is_ok_and(|row| row["cancellation_requested"] == true);
            document["status"] = json!(if requested || *control.cancellation.borrow() {
                "cancelled"
            } else if Instant::now() >= control.deadline {
                "timed_out"
            } else {
                "failed"
            });
            document["cancellation_requested"] = json!(requested);
            document["finished_at_us"] = json!(now_us());
            document["diagnostics"].as_array_mut().unwrap().push(json!(format!("failed to retain analysis evidence: {error:#}; original execution workspace was preserved")));
            let _ = self.store.save_document("analysis-results", &id, &document);
            return;
        }
        document["finished_at_us"] = json!(now_us());
        if document["status"] != "accepted" {
            let _ = self.store.save_document("analysis-results", &id, &document);
            return;
        }
        if let Ok(records) = result {
            let source = format!("analysis:{}", analysis["id"].as_str().unwrap());
            let identity = format!(
                "analysis-v1:{}:{}",
                analysis["package_digest"].as_str().unwrap(),
                input.fingerprint
            );
            if let Err(error) = self
                .store
                .publish_result(crate::publication::PublicationRequest {
                    run_id: &run_id,
                    source_key: &source,
                    input_identity: &identity,
                    records: &records,
                    metadata: Some(crate::publication::PublicationMetadata::Analysis {
                        execution_id: &id,
                        document: &document,
                    }),
                    deadline: Some(control.deadline.into_std()),
                })
            {
                let requested = self
                    .store
                    .document("analysis-results", &id)
                    .is_ok_and(|row| row["cancellation_requested"] == true);
                document["status"] = json!(if requested {
                    "cancelled"
                } else if Instant::now() >= control.deadline {
                    "timed_out"
                } else {
                    "failed"
                });
                document["cancellation_requested"] = json!(requested);
                document["diagnostics"] =
                    json!([format!("analysis output rejected atomically: {error:#}")]);
                let _ = self.store.save_document("analysis-results", &id, &document);
            }
        } else {
            let _ = self.store.save_document("analysis-results", &id, &document);
        }
    }
}

fn validate_output(
    analysis: &Value,
    input: &Input,
    output: &Path,
    control: &process::Control,
) -> Result<Vec<Value>> {
    control.check()?;
    let bytes =
        super::files::read(output, process::OUTPUT_LIMIT, || control.check()).map_err(|error| {
            if error
                .downcast_ref::<super::files::ByteLimitExceeded>()
                .is_some()
            {
                error.context("analyzer output exceeds the 16 MiB output limit")
            } else {
                error
            }
        })?;
    let text = String::from_utf8(bytes).context("analyzer output must be UTF-8 JSONL")?;
    control.check()?;
    let inputs = &input.record_ids;
    let mut records = Vec::new();
    for (line, value) in text
        .lines()
        .enumerate()
        .filter(|(_, text)| !text.trim().is_empty())
    {
        control.check()?;
        let mut record: Value = serde_json::from_str(value)
            .with_context(|| format!("invalid analyzer JSONL at line {}", line + 1))?;
        if !record.is_object() || record["basis"] != "derived" {
            bail!(
                "analyzer observations must retain derived basis (line {})",
                line + 1
            );
        }
        let evidence = record["evidence"]
            .as_array()
            .filter(|v| !v.is_empty())
            .context("every derived observation needs input record evidence")?;
        for reference in evidence {
            if !reference.as_str().is_some_and(|id| inputs.contains(id)) {
                bail!("derived evidence must reference an input observation from this run");
            }
        }
        if record.get("attributes").is_none() {
            record["attributes"] = json!({});
        }
        if !record["attributes"].is_object() {
            bail!("analyzer attributes must be an object");
        }
        record["attributes"]["hunin_analysis_id"] = analysis["id"].clone();
        record["attributes"]["analysis_package_digest"] = analysis["package_digest"].clone();
        records.push(record);
    }
    control.check()?;
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_or_cancelled_retention_cannot_publish_an_otherwise_valid_result() {
        for (cancelled, expired, oversized, status) in [
            (false, true, false, "timed_out"),
            (true, false, false, "cancelled"),
            (false, false, true, "failed"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let store = Store::open(&root.path().join("store")).unwrap();
            store
                .save_run(&crate::model::runs::Run {
                    id: "recording".into(),
                    name: "finishing gate".into(),
                    status: "completed".into(),
                    command: vec![],
                    cwd: "/fixture".into(),
                    runtime: "fixture".into(),
                    started_at_us: 1,
                    finished_at_us: Some(2),
                    exit_code: Some(0),
                    diagnostics: vec![],
                    source: json!({}),
                    correctness: json!({}),
                })
                .unwrap();
            let event = json!({"schema_version":1,"kind":"event","id":"input","run_id":"recording","producer_id":"fixture","pid":1,"tid":"thread","timestamp_us":1,"basis":"observed","name":"input"});
            store
                .accept("recording", "input", std::slice::from_ref(&event))
                .unwrap();
            let mut derived = event.clone();
            derived["id"] = json!("derived");
            derived["basis"] = json!("derived");
            derived["evidence"] = json!(["input"]);
            let directory = root.path().join("attempt");
            fs::create_dir(&directory).unwrap();
            fs::write(
                directory.join("stdout.jsonl"),
                serde_json::to_vec(&derived).unwrap(),
            )
            .unwrap();
            let document = json!({"id":"attempt","run_id":"recording","status":"running","diagnostics":[],"phase_timings_us":{},"execution_dir":directory});
            if oversized {
                fs::OpenOptions::new()
                    .write(true)
                    .open(directory.join("stdout.jsonl"))
                    .unwrap()
                    .set_len(retention::Limits::default().bytes + 1)
                    .unwrap();
            }
            store
                .save_document("analysis-results", "attempt", &document)
                .unwrap();
            let (_sender, cancellation) = watch::channel(cancelled);
            let now = Instant::now();
            let deadline = if expired {
                now - std::time::Duration::from_secs(1)
            } else {
                now + std::time::Duration::from_secs(30)
            };
            let control = process::Control::new(cancellation, deadline);
            let input = Input {
                bytes: vec![],
                fingerprint: "input-identity".into(),
                run_id: "recording".into(),
                result_ids: vec![],
                record_ids: ["input".into()].into_iter().collect(),
            };
            let engine = Engine::new(store.clone());
            engine.finish(
                &json!({"id":"analysis","package_digest":"package"}),
                &input,
                document,
                &directory,
                &control,
                Ok(vec![derived]),
            );
            let document = store.document("analysis-results", "attempt").unwrap();
            assert_eq!(document["status"], status);
            assert_eq!(document["raw_retention"]["complete"], false);
            assert_eq!(store.records("recording").unwrap(), vec![event]);
            assert!(
                store
                    .selected_result_id("recording", "analysis:analysis")
                    .unwrap()
                    .is_none()
            );
            assert!(directory.join("stdout.jsonl").is_file());
        }
    }

    #[test]
    fn output_validation_checks_actual_capture_bytes_before_parsing() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("stdout.jsonl");
        fs::File::create(&output)
            .unwrap()
            .set_len(process::OUTPUT_LIMIT + 1)
            .unwrap();
        let (_sender, cancellation) = watch::channel(false);
        let control = process::Control::new(
            cancellation,
            Instant::now() + std::time::Duration::from_secs(30),
        );
        let input = Input {
            bytes: vec![],
            fingerprint: String::new(),
            run_id: "fixture".into(),
            result_ids: vec![],
            record_ids: Default::default(),
        };
        let error = validate_output(&json!({}), &input, &output, &control).unwrap_err();
        assert!(format!("{error:#}").contains("16 MiB"));
        assert_eq!(
            fs::metadata(output).unwrap().len(),
            process::OUTPUT_LIMIT + 1
        );
    }
}

//! Canonical, size-bounded analyzer inputs from one selected-observation snapshot.
use super::{applicability, files};
use crate::storage::{RecordReadLimit, Store};
use anyhow::Result;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    io::{self, Write},
};

const INPUT_LIMIT: usize = 128 * 1024 * 1024;
const LIMIT_MESSAGE: &str = "analysis input exceeds the 128 MiB snapshot limit";

pub(super) struct Input {
    pub bytes: Vec<u8>,
    pub fingerprint: String,
    pub run_id: String,
    pub result_ids: Vec<String>,
    pub record_ids: HashSet<String>,
}

pub(super) fn snapshot(
    store: &Store,
    analysis: &Value,
    run_id: &str,
    checkpoint: impl FnMut() -> Result<()>,
) -> Result<Input> {
    snapshot_with_limit(store, analysis, run_id, INPUT_LIMIT, checkpoint)
}

fn snapshot_with_limit(
    store: &Store,
    analysis: &Value,
    run_id: &str,
    maximum_bytes: usize,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<Input> {
    checkpoint()?;
    let run = store.run(run_id)?;
    let artifacts = store
        .artifacts(run_id)?
        .into_iter()
        .filter(|artifact| !artifact.format.starts_with("analysis-"))
        .collect::<Vec<_>>();
    applicability::check(&analysis["requirements"], &run, &artifacts)?;
    let artifacts: Vec<_> = artifacts
        .into_iter()
        .map(|artifact| {
            checkpoint()?;
            let path = store.artifact_path(run_id, &artifact.id)?;
            let mut value = serde_json::to_value(artifact)?;
            value["path"] = json!(path);
            Ok(value)
        })
        .collect::<Result<_>>()?;
    let source_key = format!("analysis:{}", analysis["id"].as_str().unwrap());
    store.with_selected_snapshot(run_id, &source_key, |selected| {
        checkpoint()?;
        let header = json!({
            "run": run,
            "records": [],
            "artifacts": artifacts,
            "input_result_sets": selected.result_ids(),
            "analyzer": {
                "id": analysis["id"],
                "version": analysis["version"],
                "package_digest": analysis["package_digest"],
            },
        });
        let mut output = LimitedBuffer {
            bytes: Vec::new(),
            maximum_bytes,
        };
        let mut record_ids = HashSet::new();
        output.write_all(b"{")?;
        // Iterate the same JSON map used by the original encoder. Serde owns
        // escaping/numbers; streaming the array preserves canonical bytes.
        for (index, (key, value)) in header.as_object().unwrap().iter().enumerate() {
            checkpoint()?;
            if index > 0 {
                output.write_all(b",")?;
            }
            serde_json::to_writer(&mut output, key)?;
            output.write_all(b":")?;
            if key != "records" {
                serde_json::to_writer(&mut output, value)?;
                continue;
            }
            output.write_all(b"[")?;
            let mut first = true;
            selected
                .visit(maximum_bytes, |record| {
                    checkpoint()?;
                    if !first {
                        output.write_all(b",")?;
                    }
                    first = false;
                    serde_json::to_writer(&mut output, &record)?;
                    if let Some(id) = record["id"].as_str() {
                        record_ids.insert(id.to_owned());
                    }
                    Ok(())
                })
                .map_err(|error| {
                    if error.downcast_ref::<RecordReadLimit>().is_some() {
                        error.context(LIMIT_MESSAGE)
                    } else {
                        error
                    }
                })?;
            output.write_all(b"]")?;
        }
        output.write_all(b"}")?;
        checkpoint()?;
        Ok(Input {
            fingerprint: files::digest(&output.bytes),
            bytes: output.bytes,
            run_id: run_id.to_owned(),
            result_ids: selected.result_ids().to_owned(),
            record_ids,
        })
    })
}

struct LimitedBuffer {
    bytes: Vec<u8>,
    maximum_bytes: usize,
}
impl Write for LimitedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.maximum_bytes.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other(LIMIT_MESSAGE));
        }
        let required = self.bytes.len() + bytes.len();
        if required > self.bytes.capacity() {
            let capacity = self
                .bytes
                .capacity()
                .max(8)
                .saturating_mul(2)
                .max(required)
                .min(self.maximum_bytes);
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(io::Error::other)?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::runs::{Run, RunStatus};

    fn fixture() -> (tempfile::TempDir, Store, Value) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        store
            .save_run(&Run {
                id: "recording".into(),
                name: "input codec".into(),
                status: RunStatus::Completed,
                command: vec!["fixture".into()],
                cwd: ".".into(),
                runtime: "fixture".into(),
                started_at_us: 1,
                finished_at_us: Some(2),
                exit_code: Some(0),
                diagnostics: vec![],
                source: json!({"fingerprint":"fixture"}),
                correctness: json!({"passed":true}),
            })
            .unwrap();
        let records = [
            json!({"schema_version":1,"kind":"event","id":"input","run_id":"recording","producer_id":"fixture","pid":1,"tid":"main","timestamp_us":1,"name":"fixture","basis":"observed","attributes":{"null":null,"escaped":"\"\\\n☃","number":1.5,"large_identity":"18446744073709551615","content":"x".repeat(4096)}}),
        ];
        store.accept("recording", "source", &records).unwrap();
        let analysis = json!({"id":"fixture","version":"1","package_digest":"fixture-package","requirements":{"runtime":"fixture","source_fingerprint":"fixture"}});
        (root, store, analysis)
    }

    #[test]
    fn streamed_input_preserves_the_existing_canonical_bytes_and_fingerprint() {
        let (_root, store, analysis) = fixture();
        let expected = json!({"run":store.run("recording").unwrap(),"records":store.records("recording").unwrap(),"artifacts":[],"input_result_sets":store.active_result_ids("recording","analysis:fixture").unwrap(),"analyzer":{"id":"fixture","version":"1","package_digest":"fixture-package"}});
        let bytes = serde_json::to_vec(&expected).unwrap();
        let input =
            snapshot_with_limit(&store, &analysis, "recording", bytes.len(), || Ok(())).unwrap();
        assert_eq!(input.bytes, bytes);
        assert_eq!(input.fingerprint, files::digest(&bytes));
        assert_eq!(input.record_ids, HashSet::from(["input".to_owned()]));
        assert!(
            snapshot_with_limit(&store, &analysis, "recording", bytes.len() - 1, || Ok(()))
                .is_err()
        );
        assert_eq!(store.records("recording").unwrap().len(), 1);
    }

    #[test]
    fn an_oversized_stored_record_is_rejected_before_json_decoding() {
        let (_root, store, analysis) = fixture();
        let record_bytes = serde_json::to_vec(&store.records("recording").unwrap()[0])
            .unwrap()
            .len();
        let error =
            snapshot_with_limit(&store, &analysis, "recording", record_bytes - 1, || Ok(()))
                .err()
                .unwrap();
        assert!(
            error.downcast_ref::<RecordReadLimit>().is_some(),
            "{error:#}"
        );
        assert!(error.to_string().contains(LIMIT_MESSAGE));
    }

    #[test]
    #[ignore = "opt-in analyzer sizing on an explicitly retained synthetic store"]
    fn analyzer_input_sizing() {
        let root = std::env::var_os("HUNIN_SIZING_STORE").expect("HUNIN_SIZING_STORE required");
        let store = Store::open(std::path::Path::new(&root)).unwrap();
        let analysis = json!({"id":"sizing","version":"1","package_digest":"sizing-package","requirements":{"runtime":"synthetic","source_fingerprint":"fixture-v1"}});
        assert!(store.artifacts("sizing").unwrap().is_empty());
        let implementation =
            std::env::var("HUNIN_SIZING_IMPLEMENTATION").unwrap_or_else(|_| "streamed".into());
        let start = std::time::Instant::now();
        let (fingerprint, bytes, records) = if implementation == "legacy" {
            // The former builder: collect all Values, clone through json!,
            // serialize/hash, then retain the complete Value for the attempt.
            fn legacy(store: &Store) -> (Value, String) {
                let run = store.run("sizing").unwrap();
                let result_ids = store
                    .active_result_ids("sizing", "analysis:sizing")
                    .unwrap();
                let records = store.records_for_result_ids("sizing", &result_ids).unwrap();
                let value = json!({"run":run,"records":records,"artifacts":[],"input_result_sets":result_ids,"analyzer":{"id":"sizing","version":"1","package_digest":"sizing-package"}});
                let bytes = serde_json::to_vec(&value).unwrap();
                (value, files::digest(&bytes))
            }
            let (value, fingerprint) = legacy(&store);
            let bytes = serde_json::to_vec(&value).unwrap().len();
            let records = value["records"].as_array().unwrap().len();
            std::hint::black_box(&value);
            (fingerprint, bytes, records)
        } else {
            assert_eq!(implementation, "streamed");
            let input = snapshot(&store, &analysis, "sizing", || Ok(())).unwrap();
            (input.fingerprint, input.bytes.len(), input.record_ids.len())
        };
        println!(
            "{}",
            json!({"implementation":implementation,"fingerprint":fingerprint,
            "input_bytes":bytes,"input_records":records,"elapsed_us":start.elapsed().as_micros(),
            "source_revision":std::env::var("HUNIN_SIZING_REVISION").ok(),
            "host_os":std::env::consts::OS,"host_arch":std::env::consts::ARCH,"debug_assertions":cfg!(debug_assertions)})
        );
    }
}

use crate::collection::CaptureRequest;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
};

fn bounded(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("read count collector output {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(maximum + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= maximum,
        "function-count output exceeds {maximum} bytes"
    );
    Ok(bytes)
}

pub(in crate::collection) fn validate(
    request: &CaptureRequest,
    files: &[PathBuf],
    method: &str,
    maximum: u64,
    maximum_functions: u32,
) -> Result<()> {
    let profile = request.artifact_dir.join("calls.calls.jsonl");
    let completion = request.artifact_dir.join("calls.complete.json");
    ensure!(
        files.contains(&profile) && files.contains(&completion),
        "function-count collector did not finalize its declared profile and completion manifest"
    );
    let report: Value = serde_json::from_slice(&bounded(&completion, 64 * 1024)?)?;
    ensure!(
        report["schema_version"] == 1
            && report["run_id"] == request.run_id
            && report["method"] == method,
        "function-count completion identity does not match this recording"
    );
    ensure!(
        report["status"] == "complete",
        "function-count collection incomplete: {}",
        report
    );
    ensure!(
        report["dropped_events"].as_u64() == Some(0)
            && report["counter_overflow"] == false
            && report["output_truncated"] == false
            && report["loss"] == "none_observed",
        "function-count completion contradicts its loss or bounds report"
    );
    if method == "cpython-monitoring-native" {
        ensure!(
            report["observer_intact"] == true,
            "Python count observer ownership was not intact at finalization"
        );
    }
    let bytes = bounded(&profile, maximum)?;
    ensure!(
        report["bytes"].as_u64() == Some(bytes.len() as u64),
        "function-count output length does not match completion manifest"
    );
    if let Some(digest) = report["sha256"].as_str() {
        ensure!(
            digest == format!("{:x}", Sha256::digest(&bytes)),
            "function-count output digest does not match completion manifest"
        );
    } else {
        let digest = bytes.iter().fold(0xcbf29ce484222325u64, |value, byte| {
            (value ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
        ensure!(
            report["fnv1a64"].as_str() == Some(format!("{digest:016x}").as_str()),
            "function-count output integrity does not match completion manifest"
        );
    }
    let mut records = 0u64;
    let mut complete = false;
    let mut functions = HashSet::new();
    let mut counters = HashSet::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let record: Value = serde_json::from_slice(line)?;
        ensure!(
            !complete,
            "function-count records follow the completion sentinel"
        );
        ensure!(
            record["run_id"] == request.run_id,
            "function count belongs to a different recording"
        );
        ensure!(
            record["producer_id"] == format!("calls:{}", request.run_id),
            "function-count producer identity does not match this recording"
        );
        let attributes = &record["attributes"];
        ensure!(
            attributes["method"] == method && attributes["collection_scope"] == "all_then_filter",
            "function-count row method or collection scope differs from its capture plan"
        );
        if record["kind"] == "event" && record["name"] == "function.counts.complete" {
            ensure!(
                attributes["status"] == "complete",
                "function-count completion event is incomplete"
            );
            complete = true;
        } else {
            let name = record["name"]
                .as_str()
                .context("function-count metric name missing")?;
            ensure!(
                record["kind"] == "metric"
                    && matches!(name, "function.calls" | "function.resumes")
                    && record["basis"] == "observed"
                    && record["unit"] == "count"
                    && record["scope"] == "process",
                "unexpected row in function-count profile"
            );
            ensure!(
                attributes["count_semantics"]
                    == if name == "function.calls" {
                        "starts"
                    } else {
                        "resumes"
                    },
                "function-count metric name and semantics disagree"
            );
            ensure!(
                attributes["counter_temporality"] == "delta",
                "function count must declare delta counter temporality"
            );
            ensure!(
                record["value"]
                    .as_u64()
                    .is_some_and(|value| value > 0 && value <= 9_007_199_254_740_991),
                "function count must be a positive exact cross-language integer; absence is not zero"
            );
            let function = &attributes["function"];
            ensure!(
                function["name"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty() && value.len() <= 4096),
                "function-count identity name is missing or unbounded"
            );
            ensure!(
                function["file"].is_null()
                    || function["file"]
                        .as_str()
                        .is_some_and(|value| value.len() <= 16384),
                "function-count file identity is invalid"
            );
            ensure!(
                function["line"].is_null()
                    || function["line"]
                        .as_u64()
                        .is_some_and(|value| value > 0 && value <= u64::from(u32::MAX)),
                "function-count line identity is invalid"
            );
            if let Some(code_id) = function.get("code_id") {
                ensure!(
                    code_id
                        .as_str()
                        .is_some_and(|value| !value.is_empty() && value.len() <= 4096),
                    "function-count code identity must be a bounded string when present"
                );
            }
            let key = serde_json::to_string(function)?;
            functions.insert(key.clone());
            ensure!(
                functions.len() <= maximum_functions as usize,
                "function-count identity inventory exceeds the requested bound"
            );
            ensure!(
                counters.insert((key, name.to_owned())),
                "duplicate function counter within one count window"
            );
        }
        records += 1;
    }
    ensure!(
        report["records"].as_u64() == Some(records),
        "function-count record inventory differs from completion manifest"
    );
    ensure!(
        complete,
        "function-count profile has no matching accepted completion event"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (tempfile::TempDir, CaptureRequest, Vec<PathBuf>, Value) {
        let root = tempfile::tempdir().unwrap();
        let request = CaptureRequest {
            command: vec!["target".into()],
            cwd: root.path().into(),
            artifact_dir: root.path().into(),
            run_id: "control".into(),
            runtime: "c".into(),
            capture_mode: Default::default(),
            code_options: None,
            env: Default::default(),
        };
        let profile = root.path().join("calls.calls.jsonl");
        let manifest = root.path().join("calls.complete.json");
        let data = format!(
            "{}\n",
            json!({"kind":"event","name":"function.counts.complete","run_id":"control","producer_id":"calls:control","attributes":{"status":"complete","method":"compiler-hooks","collection_scope":"all_then_filter"}})
        );
        std::fs::write(&profile, &data).unwrap();
        let report = json!({"schema_version":1,"run_id":"control","method":"compiler-hooks","status":"complete","bytes":data.len(),"records":1,"sha256":format!("{:x}",Sha256::digest(data.as_bytes())),"dropped_events":0,"counter_overflow":false,"output_truncated":false,"loss":"none_observed"});
        std::fs::write(&manifest, serde_json::to_vec(&report).unwrap()).unwrap();
        (root, request, vec![profile, manifest], report)
    }

    #[test]
    fn complete_empty_selection_is_valid_but_mutated_output_is_not() {
        let (_root, request, files, _) = fixture();
        validate(&request, &files, "compiler-hooks", 8192, 8).unwrap();
        let data = std::fs::read_to_string(&files[0])
            .unwrap()
            .replace("calls:control", "calls:another");
        std::fs::write(&files[0], data).unwrap();
        assert!(validate(&request, &files, "compiler-hooks", 8192, 8).is_err());
    }

    #[test]
    fn a_complete_label_never_overrides_loss_or_missing_inventory() {
        let (_root, request, files, mut report) = fixture();
        report["dropped_events"] = json!(1);
        std::fs::write(&files[1], serde_json::to_vec(&report).unwrap()).unwrap();
        assert!(
            validate(&request, &files, "compiler-hooks", 8192, 8)
                .unwrap_err()
                .to_string()
                .contains("loss")
        );
        assert!(validate(&request, &files[..1], "compiler-hooks", 8192, 8).is_err());
    }

    #[test]
    fn coherent_checksums_do_not_override_metric_semantics_or_function_bounds() {
        for fault in [
            "method",
            "zero",
            "bound",
            "status",
            "temporality",
            "code_id",
        ] {
            let (_root, request, files, mut report) = fixture();
            let event: Value =
                serde_json::from_str(std::fs::read_to_string(&files[0]).unwrap().trim()).unwrap();
            let mut metric = json!({"kind":"metric","name":"function.calls","run_id":"control","producer_id":"calls:control","basis":"observed","unit":"count","scope":"process","value":3,"attributes":{"method":"compiler-hooks","collection_scope":"all_then_filter","counter_temporality":"delta","count_semantics":"starts","function":{"name":"leaf","file":null,"line":null}}});
            let mut rows = vec![metric.clone(), event];
            match fault {
                "method" => rows[0]["attributes"]["method"] = json!("other"),
                "zero" => rows[0]["value"] = json!(0),
                "bound" => {
                    metric["attributes"]["function"]["name"] = json!("other");
                    rows.insert(1, metric);
                }
                "status" => rows[1]["attributes"]["status"] = json!("incomplete"),
                "temporality" => rows[0]["attributes"]["counter_temporality"] = json!("cumulative"),
                "code_id" => rows[0]["attributes"]["function"]["code_id"] = json!(17),
                _ => unreachable!(),
            }
            let data = rows
                .iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>();
            report["bytes"] = json!(data.len());
            report["records"] = json!(rows.len());
            report["sha256"] = json!(format!("{:x}", Sha256::digest(data.as_bytes())));
            std::fs::write(&files[0], data).unwrap();
            std::fs::write(&files[1], serde_json::to_vec(&report).unwrap()).unwrap();
            assert!(
                validate(&request, &files, "compiler-hooks", 8192, 1).is_err(),
                "fault {fault} should be rejected independently of a coherent digest"
            );
        }
    }
}

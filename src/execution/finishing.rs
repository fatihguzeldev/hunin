//! Synchronous correctness, raw retention, normalization and SDK publication.
use super::{
    CaptureRequest, Engine, Run, RunStatus, collection, correctness, native, resources, sdk,
};
use crate::model::now_us;
use anyhow::Result;
use serde_json::{Value, json};

pub(super) struct Target {
    pub exit_code: Option<i32>,
    pub outcome: Option<Value>,
    pub canceled: bool,
    pub timed_out: bool,
}
pub(super) struct Capture {
    pub request: CaptureRequest,
    pub plan: collection::CapturePlan,
    pub start_failure: Option<String>,
}

impl Engine {
    pub(super) fn finish(
        &self,
        mut run: Run,
        target: Target,
        capture: Capture,
        metrics: resources::Retention,
    ) -> Result<()> {
        let Capture {
            request: capture,
            plan,
            start_failure: collector_start_failure,
        } = capture;
        let directory = capture.artifact_dir.clone();
        let stdout = directory.join("stdout.txt");
        let stderr = directory.join("stderr.txt");
        let native_collection = native::handles_runtime(&run.runtime);
        let (metrics, retention) = metrics.finish();
        if retention.truncated {
            run.diagnostics.push(format!("Resource retention safety budget reached; {} received observation rows were not retained. Target correctness is independent; see resource_observation.retention",retention.discarded_rows));
        }
        run.source["resource_observation"]["retention"] = serde_json::to_value(retention)?;
        run.exit_code = if let Some(outcome) = &target.outcome {
            if outcome["state"] == "exited" {
                outcome["exit_code"]
                    .as_i64()
                    .and_then(|code| i32::try_from(code).ok())
            } else {
                None
            }
        } else {
            target.exit_code
        };
        run.finished_at_us = Some(now_us());
        run.correctness = correctness::evaluate(
            &run.source["correctness_recipe"],
            &stdout,
            run.exit_code,
            target.canceled,
            target.timed_out,
        )?;
        if !target.canceled && !target.timed_out {
            for check in run.correctness["checks"].as_array().unwrap() {
                if check["passed"] == false {
                    run.diagnostics.push(format!(
                        "target correctness check failed: {}",
                        check["kind"].as_str().unwrap_or("unknown")
                    ));
                }
            }
        }
        run.status = if target.canceled {
            RunStatus::Canceled
        } else if run.correctness["passed"] != true {
            RunStatus::Failed
        } else {
            RunStatus::Completed
        };
        run.source["target_outcome"] = target
            .outcome
            .unwrap_or_else(|| json!({"exit_code":run.exit_code}));
        run.source["target_outcome"]["status"] = json!(run.status);
        run.source["target_outcome"]["correctness"] = run.correctness.clone();
        run.source["collection_status"] = json!(if run.runtime == "none" {
            "disabled"
        } else {
            "finalizing"
        });
        // Keep waiters on the recording until preservation and ingestion finish,
        // while retaining the actual target outcome if finalization later fails.
        let mut finalizing = run.clone();
        finalizing.status = RunStatus::Running;
        finalizing.finished_at_us = None;
        self.store.save_run(&finalizing)?;
        if target.timed_out {
            run.diagnostics
                .push("recording timeout; target process group terminated".into());
        }
        self.store.add_artifact(&run.id, &stdout, "stdout")?;
        self.store.add_artifact(&run.id, &stderr, "stderr")?;
        preserve_capture(&self.store, &run.id, &directory)?;
        if !metrics.is_empty() {
            self.store
                .accept(&run.id, "process-resources-v1", &metrics)?;
        }
        if run.runtime != "none" {
            let profiles = plan.finalize(&capture).and_then(|mut bundle| {
                if let Some(diagnostic) = &collector_start_failure {
                    bundle.completion.state = collection::CompletionState::Incomplete;
                    bundle.completion.diagnostics.push(diagnostic.clone());
                }
                run.source["collection_completion"] = serde_json::to_value(&bundle.completion)?;
                bundle.completion.ensure_complete()?;
                // Preserve the previous all-decode-before-publication behavior.
                // Raw retention above also survives incomplete or invalid profiles.
                bundle
                    .profiles
                    .iter()
                    .map(|profile| {
                        crate::importers::import(&profile.path, &profile.format, &run.id)
                    })
                    .collect::<Result<Vec<_>>>()
            });
            match profiles {
                Ok(profiles) => {
                    for profile in profiles {
                        let publication =
                            crate::recordings::publish(&self.store, &run.id, profile)?;
                        run.diagnostics.extend(publication.diagnostics);
                    }
                    run.source["collection_status"] = json!("completed");
                }
                Err(error) => {
                    run.diagnostics
                        .push(format!("automatic collection incomplete: {error:#}"));
                    run.source["collection_status"] = json!("failed");
                    if run.status == RunStatus::Completed {
                        run.status = RunStatus::Failed;
                    }
                }
            }
        }
        if native_collection
            && run.source["collector_outcome"]["exit_status_consistent_with_target"] == false
        {
            run.source["collection_status"] = json!("failed");
            run.diagnostics.push("Native collector exited differently from the observed target status; raw and any accepted profile evidence were retained, and collector failure does not replace the target outcome".into());
            if run.status == RunStatus::Completed {
                run.status = RunStatus::Failed;
            }
        }
        let sdk = sdk::ingest(&self.store, &run.id, &directory.join("sdk"));
        if let Some(diagnostic) = sdk.rejection() {
            run.diagnostics.push(diagnostic.to_owned());
            if run.status == RunStatus::Completed {
                run.status = RunStatus::Failed;
            }
        }
        run.source["sdk_ingestion"] = serde_json::to_value(sdk)?;
        run.finished_at_us = Some(now_us());
        self.store.save_run(&run)?;
        Ok(())
    }
}

fn preserve_capture(
    store: &crate::storage::Store,
    run_id: &str,
    path: &std::path::Path,
) -> Result<()> {
    for artifact in collection::capture_artifacts(path)? {
        store.add_artifact(run_id, &artifact.path, &artifact.format)?;
    }
    Ok(())
}

//! Synchronous correctness, raw retention, normalization and SDK publication.
use super::{Engine, correctness, resources, retention, sdk};
use crate::{
    collection::{self, CaptureRequest, native},
    model::{
        now_us,
        runs::{Run, RunStatus},
    },
};
use anyhow::Result;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

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

struct Policy {
    raw: retention::Limits,
    maximum_stdout_bytes: u64,
    checkpoint_duration: Duration,
}
impl Default for Policy {
    fn default() -> Self {
        let raw = retention::Limits::default();
        Self {
            maximum_stdout_bytes: raw.file_bytes,
            raw,
            checkpoint_duration: Duration::from_secs(120),
        }
    }
}

fn checkpoint(deadline: Instant) -> Result<()> {
    anyhow::ensure!(
        Instant::now() < deadline,
        "recording finishing checkpoint deadline elapsed"
    );
    Ok(())
}

impl Engine {
    pub(super) fn finish(
        &self,
        run: Run,
        target: Target,
        capture: Capture,
        metrics: resources::Retention,
    ) -> Result<()> {
        self.finish_with_raw_limits(run, target, capture, metrics, retention::Limits::default())
    }

    fn finish_with_raw_limits(
        &self,
        run: Run,
        target: Target,
        capture: Capture,
        metrics: resources::Retention,
        raw_limits: retention::Limits,
    ) -> Result<()> {
        self.finish_with_policy(
            run,
            target,
            capture,
            metrics,
            Policy {
                raw: raw_limits,
                ..Policy::default()
            },
        )
    }

    fn finish_with_policy(
        &self,
        mut run: Run,
        target: Target,
        capture: Capture,
        metrics: resources::Retention,
        policy: Policy,
    ) -> Result<()> {
        let began = Instant::now();
        let deadline = began + policy.checkpoint_duration;
        let state = |phase: &str, status: &str| {
            json!({
                "policy": "recording-finishing-v1",
                "status": status,
                "phase": phase,
                "checkpoint_budget_ms": policy.checkpoint_duration.as_millis().min(u64::MAX as u128) as u64,
                "elapsed_before_terminal_write_ms": began.elapsed().as_millis().min(u64::MAX as u128) as u64,
                "scope": "synchronous finishing worker after owned process teardown and resource observer join",
                "limitations": concat!(
                    "Cooperative checks do not preempt one filesystem, parser, external-tool or SQLite unit. ",
                    "Terminal failure evidence remains writable after expiry; no new profile/SDK result ",
                    "starts publication after its deadline checkpoint. Earlier independently accepted ",
                    "complete result sets remain evidence. This is not a total process memory quota ",
                    "or a hard shutdown duration."
                ),
            })
        };
        let Capture {
            request: capture,
            plan,
            start_failure: collector_start_failure,
        } = capture;
        let directory = capture.artifact_dir.clone();
        let stdout = directory.join("stdout.txt");
        let native_collection = native::handles_runtime(&run.runtime)
            && !matches!(
                plan.output,
                collection::CaptureOutput::FunctionCounts { .. }
            );
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
        run.source["target_outcome"] = target
            .outcome
            .clone()
            .unwrap_or_else(|| json!({"exit_code":run.exit_code}));
        run.source["target_outcome"]["status"] = json!(if target.canceled {
            "canceled"
        } else if target.timed_out {
            "timed_out"
        } else if run.exit_code.is_some() {
            "exited"
        } else {
            "unknown"
        });
        run.source["finishing"] = state("correctness", "running");
        let mut evidence = run.clone();
        evidence.status = RunStatus::Running;
        evidence.finished_at_us = None;
        self.store.save_run(&evidence)?;
        let mut finishing_incomplete = false;
        run.correctness = match correctness::evaluate(
            &run.source["correctness_recipe"],
            &stdout,
            run.exit_code,
            target.canceled,
            target.timed_out,
            policy.maximum_stdout_bytes,
            || checkpoint(deadline),
        ) {
            Ok(correctness) => correctness,
            Err(error)
                if error.is::<crate::host::FileReadLimit>() || Instant::now() >= deadline =>
            {
                finishing_incomplete = true;
                run.diagnostics.push(format!(
                    "Full stdout digest unavailable within finishing limits; inspect capture and artifact evidence: {error:#}"
                ));
                correctness::unavailable(
                    &run.source["correctness_recipe"],
                    run.exit_code,
                    target.canceled,
                    target.timed_out,
                    &error,
                )
            }
            Err(error) => {
                finishing_incomplete = true;
                run.diagnostics.push(format!("Stdout correctness evidence unavailable after an I/O failure; inspect capture/artifact state: {error:#}"));
                correctness::unavailable(
                    &run.source["correctness_recipe"],
                    run.exit_code,
                    target.canceled,
                    target.timed_out,
                    &error,
                )
            }
        };
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
        run.source["target_outcome"]["status"] = if run.correctness["passed"].is_null() {
            json!("validation_unknown")
        } else {
            json!(run.status)
        };
        run.source["target_outcome"]["correctness"] = run.correctness.clone();
        if finishing_incomplete && run.status == RunStatus::Completed {
            run.status = RunStatus::Failed;
        }
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
        let mut raw_limits = policy.raw;
        raw_limits.wall_time_ms = raw_limits.wall_time_ms.min(
            deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u64::MAX as u128) as u64,
        );
        run.source["finishing"] = state("raw_retention", "running");
        let preserved = retention::preserve(&self.store, &run.id, &directory, raw_limits);
        run.source["raw_retention"] = serde_json::to_value(&preserved.report)?;
        let mut raw_finalizing = run.clone();
        raw_finalizing.status = RunStatus::Running;
        raw_finalizing.finished_at_us = None;
        self.store.save_run(&raw_finalizing)?;
        if !metrics.is_empty()
            && let Err(error) = self
                .store
                .publish_result(crate::publication::PublicationRequest {
                    run_id: &run.id,
                    source_key: "process-resources-v1",
                    input_identity: "process-resources-v1",
                    records: &metrics,
                    metadata: None,
                    deadline: Some(deadline),
                })
        {
            finishing_incomplete = true;
            run.diagnostics.push(format!("Resource observation publication rejected; target outcome remains independent: {error:#}"));
            run.source["resource_observation"]["publication"] =
                json!({"status":"rejected","diagnostic":format!("{error:#}")});
            if run.status == RunStatus::Completed {
                run.status = RunStatus::Failed;
            }
        }
        if !preserved.report.complete {
            run.diagnostics.push("Raw capture retention is incomplete; retention did not remove original capture files, and complete earlier artifact references remain available. Automatic profile normalization and SDK ingestion were skipped; see raw_retention".into());
            run.source["collection_status"] = json!("failed");
            run.source["sdk_ingestion"] = json!({"status":"skipped","reason":"raw_retention_incomplete","completion":"unknown","loss":"unknown"});
            if run.status == RunStatus::Completed {
                run.status = RunStatus::Failed;
            }
            run.finished_at_us = Some(now_us());
            run.source["finishing"] = state("raw_retention", "incomplete");
            self.store.save_run(&run)?;
            return Ok(());
        }
        if run.runtime != "none" {
            run.source["finishing"] = state("normalization", "running");
            let profiles = checkpoint(deadline)
                .and_then(|_| {
                    plan.finalize_inventory_controlled(
                        &capture,
                        preserved.artifacts,
                        Some(deadline),
                    )
                })
                .and_then(|mut bundle| {
                    checkpoint(deadline)?;
                    if let Some(diagnostic) = &collector_start_failure {
                        bundle.completion.state = collection::CompletionState::Incomplete;
                        bundle.completion.diagnostics.push(diagnostic.clone());
                    }
                    run.source["collection_completion"] = serde_json::to_value(&bundle.completion)?;
                    bundle.completion.ensure_complete()?;
                    // Preserve all-decode-before-publication while charging one
                    // output budget across every retained profile in this bundle.
                    let mut output_budget = crate::importers::OutputBudget::default();
                    bundle
                        .profiles
                        .iter()
                        .map(|profile| {
                            crate::importers::import_with_budget_controlled(
                                &profile.path,
                                &profile.format,
                                &run.id,
                                &mut output_budget,
                                Some(deadline),
                            )
                        })
                        .collect::<Result<Vec<_>>>()
                });
            match profiles {
                Ok(profiles) => {
                    let publication = profiles.into_iter().try_for_each(|profile| {
                        checkpoint(deadline)?;
                        let publication = crate::recordings::publish_controlled(
                            &self.store,
                            &run.id,
                            profile,
                            Some(deadline),
                        )?;
                        run.diagnostics.extend(publication.diagnostics);
                        Ok::<_, anyhow::Error>(())
                    });
                    match publication {
                        Ok(()) => run.source["collection_status"] = json!("completed"),
                        Err(error) => {
                            finishing_incomplete = true;
                            run.diagnostics.push(format!("Profile publication incomplete; earlier fully accepted result sets remain evidence: {error:#}"));
                            run.source["collection_status"] = json!("failed");
                            if run.status == RunStatus::Completed {
                                run.status = RunStatus::Failed;
                            }
                        }
                    }
                }
                Err(error) => {
                    finishing_incomplete = true;
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
            finishing_incomplete = true;
            run.source["collection_status"] = json!("failed");
            run.diagnostics.push("Native collector exited differently from the observed target status; raw and any accepted profile evidence were retained, and collector failure does not replace the target outcome".into());
            if run.status == RunStatus::Completed {
                run.status = RunStatus::Failed;
            }
        }
        run.source["finishing"] = state("sdk_ingestion", "running");
        let sdk =
            sdk::ingest_controlled(&self.store, &run.id, &directory.join("sdk"), Some(deadline));
        if let Some(diagnostic) = sdk.rejection() {
            finishing_incomplete = true;
            run.diagnostics.push(diagnostic.to_owned());
            if run.status == RunStatus::Completed {
                run.status = RunStatus::Failed;
            }
        }
        run.source["sdk_ingestion"] = serde_json::to_value(sdk)?;
        if let Err(error) = checkpoint(deadline) {
            run.diagnostics.push(format!("Finishing budget exceeded; original target outcome and evidence remain available: {error:#}"));
            run.source["finishing"] = state("terminal_metadata", "incomplete");
            if run.status == RunStatus::Completed {
                run.status = RunStatus::Failed;
            }
        } else {
            run.source["finishing"] = state(
                "terminal_metadata",
                if finishing_incomplete {
                    "incomplete"
                } else {
                    "completed"
                },
            );
        }
        run.finished_at_us = Some(now_us());
        self.store.save_run(&run)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

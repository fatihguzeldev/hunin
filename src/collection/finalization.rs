use super::{CapturePlan, CaptureRequest, javascript, profile_format, python};
use crate::host::walk_files;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What this adapter promises to finalize, rather than a claim about all target work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CaptureOutput {
    #[default]
    Unspecified,
    Profiles {
        format: String,
    },
    FunctionCounts {
        method: String,
        maximum_bytes: u64,
    },
    GraalCpuSampler,
    DenoWorkers {
        producer_id: String,
    },
    BunProducers {
        session_id: String,
        root_registration_id: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureArtifact {
    pub path: PathBuf,
    pub format: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawProfile {
    pub path: PathBuf,
    pub format: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureProducer {
    pub id: String,
    pub process_id: u64,
    pub runtime_id: String,
    pub profiles: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ProducerCoverage {
    pub expected: Vec<String>,
    pub observed: Vec<CaptureProducer>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionState {
    Complete,
    Incomplete,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureCompletion {
    pub state: CompletionState,
    pub scope: String,
    /// None means the adapter did not establish a producer inventory.
    pub expected_producers: Option<Vec<String>>,
    pub observed_producers: Option<Vec<CaptureProducer>>,
    pub diagnostics: Vec<String>,
    pub limitations: Vec<String>,
}

impl CaptureCompletion {
    pub fn ensure_complete(&self) -> Result<()> {
        if matches!(self.state, CompletionState::Incomplete) {
            bail!("{}", self.diagnostics.join("; "));
        }
        Ok(())
    }
}

/// Raw evidence and collection conditions; no decoding or accepted observations.
#[derive(Debug, Clone, Serialize)]
pub struct RawCaptureBundle {
    pub artifacts: Vec<CaptureArtifact>,
    pub profiles: Vec<RawProfile>,
    pub completion: CaptureCompletion,
}

impl CapturePlan {
    /// Let an owned adapter retain facts observed at process spawn.
    pub fn on_started(&self, request: &CaptureRequest, pid: u32) -> Result<()> {
        match self.output {
            CaptureOutput::GraalCpuSampler => {
                python::graal::started(request, pid, self.runtime_version.as_deref())
            }
            _ => Ok(()),
        }
    }

    pub fn finalize(&self, request: &CaptureRequest) -> Result<RawCaptureBundle> {
        let artifacts = capture_artifacts(&request.artifact_dir)?;
        self.finalize_inventory(request, artifacts)
    }

    /// Finalize exactly the inventory already selected and retained by the host.
    pub(crate) fn finalize_inventory(
        &self,
        request: &CaptureRequest,
        artifacts: Vec<CaptureArtifact>,
    ) -> Result<RawCaptureBundle> {
        self.finalize_inventory_controlled(request, artifacts, None)
    }

    pub(crate) fn finalize_inventory_controlled(
        &self,
        request: &CaptureRequest,
        artifacts: Vec<CaptureArtifact>,
        deadline: Option<std::time::Instant>,
    ) -> Result<RawCaptureBundle> {
        let files = artifacts
            .iter()
            .map(|artifact| artifact.path.clone())
            .collect::<Vec<_>>();
        let mut diagnostics = Vec::new();
        let mut reads = javascript::ValidationReads::new(deadline);
        let mut coverage = None;
        let (format, scope, limitations) = match &self.output {
            CaptureOutput::Unspecified => {
                diagnostics.push("collector output contract is unspecified".into());
                (None, "unspecified", vec!["No output or producer completeness was established".into()])
            }
            CaptureOutput::Profiles { format } => (
                Some(format.as_str()),
                "declared_profile_artifacts",
                vec!["Profile artifact presence does not establish an inventory of every target process, thread or worker".into()],
            ),
            CaptureOutput::FunctionCounts { method, maximum_bytes } => {
                if let Err(error) = super::counts::validate(request, &files, method, *maximum_bytes) {
                    diagnostics.push(format!("{error:#}"));
                }
                (Some("hunin-jsonl"), "declared_function_counter_window", vec!["Completeness describes the declared counter window and supported code scope; absent functions are not zero counts".into()])
            }
            CaptureOutput::GraalCpuSampler => {
                if let Err(error) = python::graal::validate_output(request, &files) {
                    diagnostics.push(format!("{error:#}"));
                }
                (Some("graal-cpusampler"), python::graal::COMPLETION_SCOPE, vec![
                    "Only declared CPUSampler output and the owned runtime process identity are checked; this does not establish an inventory of child processes or sampling coverage of every thread".into(),
                    "Graal thread labels remain guest-thread labels rather than OS thread IDs; profiling and runtime startup contribute overhead".into(),
                    "Graal CPUSampler writes its output-file diagnostic to target stdout; no byte-exact stdout non-interference is claimed".into(),
                ])
            }
            CaptureOutput::DenoWorkers { producer_id } => {
                match javascript::finalize_deno(&files, request, producer_id, &mut reads) {
                    Ok(value) => coverage = Some(value),
                    Err(error) => diagnostics.push(format!("{error:#}")),
                }
                (Some("v8-cpuprofile"), "declared_profiles_and_registered_deno_workers", vec![
                    "Only main-isolate Web Workers registered by the adapter are enumerated; nested workers, unrelated Node workers and child processes are outside this inventory".into(),
                    "Collection completeness does not prove profile decoding, clock mapping or coverage of every target activity".into(),
                ])
            }
            CaptureOutput::BunProducers { session_id, root_registration_id } => {
                match javascript::finalize_bun(&files, request, session_id, root_registration_id, &mut reads) {
                    Ok(value) => coverage = Some(value),
                    Err(error) => diagnostics.push(format!("{error:#}")),
                }
                (Some("v8-cpuprofile"), "observed_bun_producers_and_registered_workers", vec![
                    "The planned root, observed producer manifests and constructor-registered workers are checked; child processes or workers that never expose either a registration or producer evidence are not proven covered".into(),
                    "Forced termination can leave a producer incomplete; complete artifact delivery does not establish sampling coverage or bound profiling overhead".into(),
                ])
            }
        };
        if let Err(error) = reads.check() {
            let diagnostic = format!("{error:#}");
            if !diagnostics.contains(&diagnostic) {
                diagnostics.push(diagnostic);
            }
        }
        let profiles = artifacts
            .iter()
            .filter_map(|artifact| {
                if matches!(self.output, CaptureOutput::FunctionCounts { .. })
                    && artifact.path != request.artifact_dir.join("calls.calls.jsonl")
                {
                    return None;
                }
                let profile_format = profile_format(&artifact.path)?;
                (Some(profile_format) == format).then(|| RawProfile {
                    path: artifact.path.clone(),
                    format: profile_format.into(),
                })
            })
            .collect::<Vec<_>>();
        if let Some(format) = format.filter(|_| profiles.is_empty()) {
            diagnostics.push(format!(
                "collector produced no supported profile in {}; expected {}",
                request.artifact_dir.display(),
                format
            ));
        }
        let (expected_producers, observed_producers) = match coverage {
            Some(value) => (Some(value.expected), Some(value.observed)),
            None => (None, None),
        };
        Ok(RawCaptureBundle {
            artifacts,
            profiles,
            completion: CaptureCompletion {
                state: if diagnostics.is_empty() {
                    CompletionState::Complete
                } else {
                    CompletionState::Incomplete
                },
                scope: scope.into(),
                expected_producers,
                observed_producers,
                diagnostics,
                limitations,
            },
        })
    }
}

/// Preserve these artifacts before checking completeness or decoding profile bytes.
pub fn capture_artifacts(directory: &Path) -> Result<Vec<CaptureArtifact>> {
    let mut files = Vec::new();
    walk_files(directory, &mut files)?;
    files.sort();
    Ok(artifacts_from_paths(files))
}

pub(crate) fn artifacts_from_paths(files: Vec<PathBuf>) -> Vec<CaptureArtifact> {
    files
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy();
            let format = profile_format(&path)
                .unwrap_or_else(|| {
                    if name.ends_with(".jsonl") {
                        "hunin-jsonl"
                    } else if name.ends_with(".meta.json")
                        || name.ends_with(".syms.json")
                        || name == "go-tool.json"
                    {
                        "profile-sidecar"
                    } else if name == "stdout.txt" {
                        "stdout"
                    } else if name == "stderr.txt" {
                        "stderr"
                    } else {
                        "collector-artifact"
                    }
                })
                .to_owned();
            CaptureArtifact { path, format }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{collections::BTreeMap, fs};

    fn request(directory: &Path) -> CaptureRequest {
        CaptureRequest {
            command: vec!["node".into(), "target.ts".into()],
            cwd: directory.into(),
            artifact_dir: directory.into(),
            run_id: "recording".into(),
            runtime: "node".into(),
            env: BTreeMap::new(),
        }
    }

    fn plan(output: serde_json::Value) -> CapturePlan {
        serde_json::from_value(
            json!({ "command": ["node", "target.ts"], "runtime_version": null, "output": output }),
        )
        .unwrap()
    }

    #[test]
    fn expired_controlled_finalization_never_claims_complete_delivery() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cpu.cpuprofile");
        fs::write(&path, b"original raw profile").unwrap();
        let artifacts = capture_artifacts(directory.path()).unwrap();
        let bundle = plan(json!({"kind":"profiles","format":"v8-cpuprofile"}))
            .finalize_inventory_controlled(
                &request(directory.path()),
                artifacts,
                Some(std::time::Instant::now()),
            )
            .unwrap();
        assert!(bundle.completion.ensure_complete().is_err());
        assert_eq!(bundle.artifacts.len(), 1);
        assert!(
            bundle
                .completion
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("deadline"))
        );
        assert_eq!(fs::read(path).unwrap(), b"original raw profile");
    }

    #[test]
    fn unrelated_profile_does_not_satisfy_the_adapter_contract_but_is_retained() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("unrelated.pprof"), b"raw evidence").unwrap();
        let bundle = plan(json!({"kind":"profiles", "format":"v8-cpuprofile"}))
            .finalize(&request(directory.path()))
            .unwrap();
        assert!(bundle.completion.ensure_complete().is_err());
        assert_eq!(bundle.artifacts.len(), 1);
        assert!(bundle.profiles.is_empty());
        assert!(bundle.completion.expected_producers.is_none());
    }

    #[test]
    fn artifact_delivery_does_not_decode_or_claim_complete_producer_coverage() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("cpu.cpuprofile"),
            b"opaque profiler bytes",
        )
        .unwrap();
        let bundle = plan(json!({"kind":"profiles", "format":"v8-cpuprofile"}))
            .finalize(&request(directory.path()))
            .unwrap();
        bundle.completion.ensure_complete().unwrap();
        assert_eq!(bundle.profiles.len(), 1);
        assert!(bundle.completion.expected_producers.is_none());
        assert!(bundle.completion.observed_producers.is_none());
        assert!(!bundle.completion.limitations.is_empty());
    }

    #[test]
    fn supplied_retained_inventory_is_not_silently_rescanned() {
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path().join("cpu.cpuprofile");
        fs::write(&profile, b"opaque profiler bytes").unwrap();
        let selected = capture_artifacts(directory.path()).unwrap();
        fs::write(
            directory.path().join("later.cpuprofile"),
            b"unretained later bytes",
        )
        .unwrap();
        let bundle = plan(json!({"kind":"profiles", "format":"v8-cpuprofile"}))
            .finalize_inventory(&request(directory.path()), selected)
            .unwrap();
        assert_eq!(bundle.profiles.len(), 1);
        assert_eq!(bundle.profiles[0].path, profile);
        assert_eq!(bundle.artifacts.len(), 1);
    }

    #[test]
    fn missing_planned_lifecycle_cannot_be_satisfied_by_another_report_or_profile() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("cpu.cpuprofile"), b"{}").unwrap();
        fs::write(directory.path().join("deno-workers-unrelated.jsonl"), b"{}").unwrap();
        let bundle = plan(json!({"kind":"deno-workers", "producer_id":"expected-session"}))
            .finalize(&request(directory.path()))
            .unwrap();
        assert!(
            bundle
                .completion
                .ensure_complete()
                .unwrap_err()
                .to_string()
                .contains("expected-session")
        );
        assert_eq!(bundle.artifacts.len(), 2);
        assert_eq!(bundle.profiles.len(), 1);
    }

    #[test]
    fn finalized_deno_workers_report_only_the_validated_registration_scope() {
        use sha2::{Digest, Sha256};
        let directory = tempfile::tempdir().unwrap();
        let filename = "CPU.123456.123.worker-1.cpuprofile";
        let profile_path = directory.path().join(filename);
        fs::write(&profile_path, b"{}").unwrap();
        let common = json!({"schema_version":1,"runtime":"deno","run_id":"recording","producer_id":"session"});
        let mut planned = common.clone();
        planned["status"] = json!("planned");
        let mut registered = common.clone();
        registered["pid"] = json!(123);
        registered["status"] = json!("pending");
        registered["event"] = json!("worker_registered");
        registered["registration_id"] = json!("worker");
        let mut finished = common;
        finished["pid"] = json!(123);
        finished["status"] = json!("complete");
        finished["event"] = json!("finished");
        finished["diagnostics"] = json!([]);
        finished["workers"] = json!([{"registration_id":"worker","worker_id":1}]);
        finished["profiles"] = json!([{"worker_id":1,"filename":filename,"sha256":format!("{:x}",Sha256::digest(b"{}"))}]);
        fs::write(
            directory.path().join("deno-workers-session.jsonl"),
            [planned, registered, finished]
                .iter()
                .map(|value| format!("{value}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let bundle = plan(json!({"kind":"deno-workers", "producer_id":"session"}))
            .finalize(&request(directory.path()))
            .unwrap();
        bundle.completion.ensure_complete().unwrap();
        assert_eq!(bundle.completion.expected_producers.unwrap(), ["worker"]);
        let observed = bundle.completion.observed_producers.unwrap();
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].id, "worker");
        assert_eq!(observed[0].runtime_id, "1");
        assert_eq!(observed[0].process_id, 123);
        assert_eq!(observed[0].profiles, [profile_path]);
    }
}

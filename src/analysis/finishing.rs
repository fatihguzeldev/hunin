//! Terminal attempt policy: retain evidence before publishing accepted output.
use super::{Engine, input::Input, process, retention};
use crate::model::now_us;
use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;
use tokio::time::Instant;

impl Engine {
    pub(super) fn finish(
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

#[cfg(test)]
mod tests;

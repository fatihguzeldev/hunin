//! Offline recording import, failure retention and accepted result publication.
use super::{Operations, reads};
use crate::{
    model::{now_us, runs::Run},
    storage::Store,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::path::PathBuf;

impl Operations {
    pub(super) fn import(&self, params: &Value) -> Result<Value> {
        let deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        let store = self.engine.store();
        let path = PathBuf::from(params["path"].as_str().unwrap()).canonicalize()?;
        anyhow::ensure!(path.is_file(), "recording import requires a regular file");
        let format = params["format"].as_str().unwrap();
        let run = if let Some(id) = params["run_id"].as_str() {
            let run = store.run(id)?;
            anyhow::ensure!(
                run.status != "running",
                "cannot import into a running recording"
            );
            run
        } else {
            let run = Run {
                id: uuid::Uuid::new_v4().to_string(),
                name: params["name"].as_str().unwrap_or("Imported recording").into(),
                status: "failed".into(),
                command: vec![],
                cwd: path.parent().unwrap().display().to_string(),
                runtime: "imported".into(),
                started_at_us: now_us(),
                finished_at_us: Some(now_us()),
                exit_code: None,
                diagnostics: vec!["Imported data retains its original collection limits; execution correctness is unknown".into()],
                source: json!({"fingerprint":"unknown","build_source_verified":false}),
                correctness: json!({"passed":null,"reason":"imported"}),
            };
            store.save_run(&run)?;
            run
        };
        let imported = (|| {
            store.add_artifact_controlled(
                &run.id,
                &path,
                format,
                crate::importers::NormalizationInputLimits::default().raw_bytes,
                &mut 0,
                || crate::publication::check_deadline(deadline),
            )?;
            let profile = crate::importers::import_controlled(&path, format, &run.id, deadline)?;
            crate::recordings::publish_controlled(store, &run.id, profile, deadline)
        })();
        let publication = match imported {
            Ok(publication) => publication,
            Err(error) => {
                if params["run_id"].is_null() {
                    let diagnostic =
                        format!("Import failed; any preserved inputs remain available: {error:#}");
                    store.finish_recording_import(&run.id, &[diagnostic], false)?;
                }
                return Err(error);
            }
        };
        let run = store.finish_recording_import(
            &run.id,
            &publication.diagnostics,
            params["run_id"].is_null(),
        )?;
        let mut result = reads::summary_after_mutation(store, &run.id, deadline);
        result["run"] = json!(run);
        result["accepted"] = json!(publication.accepted);
        Ok(result)
    }
}

/// Run after aggregate-work admission; its deadline starts at this workflow.
pub(super) fn reanalyze(store: &Store, run_id: &str, artifact_id: &str) -> Result<Value> {
    let deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
    let publication =
        crate::recordings::reanalyze_controlled(store, run_id, artifact_id, deadline)?;
    let mut result = reads::summary_after_mutation(store, run_id, deadline);
    result["accepted"] = json!(publication.accepted);
    result["diagnostics"] = json!(publication.diagnostics);
    Ok(result)
}

//! One analyzer attempt: verify inputs/package, prepare and execute, then finish.
use super::{
    Engine, blocking, dependencies,
    input::{self, Input},
    output, package, process,
};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{sync::watch, time::Instant};

impl Engine {
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
            blocking(move || output::validate(&package, &selected, &output, &check)).await?;
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
}

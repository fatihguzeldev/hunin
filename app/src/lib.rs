use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hunin_core::control::{Action, Controller, RequestId, RequestOutcome};
use hunin_core::execution::PreparedProcess;
use hunin_core::project::{Project, Reference, TargetRef};
use hunin_core::records::SourceHandle;
use hunin_platforms::read_resources;
use hunin_runtimes::RuntimeKind;

mod backend;
mod resources;
use backend::Backend;
pub use backend::BrowserCapturePlan;
use resources::ProcessResources;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("core: {0}")]
    Core(#[from] hunin_core::project::Error),
    #[error("execution: {0}")]
    Execution(#[from] hunin_core::execution::ExecutionError),
    #[error("runtime: {0}")]
    Runtime(#[from] hunin_runtimes::Error),
    #[error("platform: {0}")]
    Platform(#[from] hunin_platforms::process::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("capture: {0}")]
    Invalid(&'static str),
}

pub struct CapturePlan {
    pub request: RequestId,
    pub run: Reference,
    pub runtime: RuntimeKind,
    pub executable: PathBuf,
    pub workload: PathBuf,
    pub conditions: Value,
    pub environment: BTreeMap<String, String>,
    pub read_paths: Vec<PathBuf>,
    pub write_paths: Vec<PathBuf>,
    pub xdebug_extension: Option<PathBuf>,
}

pub enum BeginCapture {
    Started(Box<Capture>),
    Existing(Box<RequestOutcome>),
}

pub struct CaptureReport {
    pub request: RequestOutcome,
    pub target: TargetRef,
    pub targets: Vec<TargetRef>,
    pub collection_source: String,
    pub resource_sources: Vec<String>,
    pub code_sources: Vec<String>,
    pub application: Value,
    pub elapsed_ns: String,
    pub cpu_ns: String,
    pub collection_complete: bool,
    pub application_success: bool,
    pub resource_observations: usize,
    pub code_records: usize,
    pub warnings: Vec<String>,
}

pub struct Capture {
    project: Project,
    controller: Controller,
    request: RequestId,
    generation: i64,
    target: TargetRef,
    process: PreparedProcess,
    backend: Backend,
    area: Option<tempfile::TempDir>,
    resources: Vec<ProcessResources>,
    code: SourceHandle,
    derived_sources: Vec<SourceHandle>,
    warnings: Vec<String>,
    released: bool,
    finalized: bool,
    cleanup_confirmed: bool,
    import_complete: bool,
    retained_area: Option<PathBuf>,
    population_complete: bool,
}

fn file_digest(path: &Path) -> Result<String, Error> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn begin(project: Project, plan: CapturePlan) -> Result<BeginCapture, Error> {
    if !plan.executable.is_absolute() || !plan.workload.is_absolute() {
        return Err(Error::Invalid(
            "capture requires absolute executable and workload paths",
        ));
    }
    let intent = json!({"runtime":format!("{:?}",plan.runtime),"executable":plan.executable,"workload":plan.workload,"conditions":plan.conditions,"environment":plan.environment,"read_paths":plan.read_paths,"write_paths":plan.write_paths,"xdebug_extension":plan.xdebug_extension,"ownership":"new process group or job and its launched descendants"});
    let mut artifacts = vec![
        ("executable", plan.executable.clone()),
        ("workload", plan.workload.clone()),
    ];
    if let Some(extension) = &plan.xdebug_extension {
        artifacts.push(("xdebug", extension.clone()));
    }
    begin_with(
        project,
        &plan.request,
        &plan.run,
        intent,
        artifacts,
        |area| Backend::runtime(&plan, area),
    )
}

pub fn begin_browser(project: Project, plan: BrowserCapturePlan) -> Result<BeginCapture, Error> {
    if ![&plan.browser, &plan.node, &plan.workload]
        .iter()
        .all(|path| path.is_absolute())
    {
        return Err(Error::Invalid(
            "browser capture needs absolute executable and workload paths",
        ));
    }
    let intent = json!({"browser_kind":format!("{:?}",plan.kind),"browser":plan.browser,"node":plan.node,"driver":plan.driver,"driver_script":plan.driver_script,"workload":plan.workload,"conditions":plan.conditions,"environment":plan.environment,"read_paths":plan.read_paths,"write_paths":plan.write_paths,"safari_scripts":plan.safari_scripts,"babel_root":plan.babel_root,"babel_node":plan.babel_node,"ownership":"owned observer process group or job; shared Safari application remains borrowed; resource scope does not imply page cpu ownership"});
    let mut artifacts = vec![
        ("browser", plan.browser.clone()),
        ("node", plan.node.clone()),
        ("workload", plan.workload.clone()),
    ];
    if let Some(driver) = &plan.driver {
        artifacts.push(("browser_driver", driver.clone()));
    }
    if let Some(driver) = &plan.driver_script {
        artifacts.push(("agent_driver", driver.clone()));
    }
    begin_with(
        project,
        &plan.request,
        &plan.run,
        intent,
        artifacts,
        |area| Backend::browser(&plan, area),
    )
}

fn begin_with(
    project: Project,
    request: &RequestId,
    run: &Reference,
    intent: Value,
    artifacts: Vec<(&str, PathBuf)>,
    prepare: impl FnOnce(&Path) -> Result<Backend, Error>,
) -> Result<BeginCapture, Error> {
    let controller = Controller::new(project.clone());
    let accepted = controller.accept(request, run, Action::Launch, intent)?;
    if !accepted.newly_accepted {
        return Ok(BeginCapture::Existing(Box::new(accepted.outcome)));
    }
    let dispatched = controller.dispatch(request)?;
    prepare_capture(
        project,
        &controller,
        request,
        run,
        dispatched.generation,
        artifacts,
        prepare,
    )
    .map(|capture| BeginCapture::Started(Box::new(capture)))
}

struct Preparation {
    controller: Controller,
    request: RequestId,
    generation: i64,
    process: Option<PreparedProcess>,
    area: Option<tempfile::TempDir>,
    retained_area: Option<PathBuf>,
    sources: Vec<SourceHandle>,
    reason: String,
    published: bool,
}

impl Drop for Preparation {
    fn drop(&mut self) {
        if !self.published {
            let _ = abort_capture(
                &self.controller,
                &self.request,
                self.generation,
                self.process.as_mut(),
                &self.sources,
                (&mut self.area, &mut self.retained_area),
                CaptureFailure {
                    reason: &self.reason,
                    input: None,
                },
            );
        }
    }
}

fn prepare_capture(
    project: Project,
    controller: &Controller,
    request: &RequestId,
    run: &Reference,
    generation: i64,
    artifacts: Vec<(&str, PathBuf)>,
    prepare: impl FnOnce(&Path) -> Result<Backend, Error>,
) -> Result<Capture, Error> {
    let mut preparation = Preparation {
        controller: controller.clone(),
        request: request.clone(),
        generation,
        process: None,
        area: None,
        retained_area: None,
        sources: Vec::new(),
        reason: "capture preparation ended before publication".into(),
        published: false,
    };
    let result: Result<Capture, Error> = (|| {
        let area = tempfile::Builder::new()
            .prefix("capture-")
            .tempdir_in(project.directory().join("tmp"))?;
        let path = area.path().to_owned();
        preparation.area = Some(area);
        let mut hashes = BTreeMap::new();
        for (name, path) in artifacts {
            hashes.insert(name, file_digest(&path)?);
        }
        let backend = prepare(&path)?;
        preparation.process = Some(hunin_platforms::native::control::spawn_prepared(
            backend.launch(),
            hunin_platforms::native::control::OwnedScope::ProcessGroup,
        )?);
        let process = preparation.process.as_mut().expect("launched process");
        process.await_ready(Duration::from_secs(10))?;
        let (instances, mut warnings, population) = backend.instances(process.id())?;
        let prefix = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(
                &json!({"caller":request.caller,"id":request.id})
            )?)
        );
        let mut resources = Vec::new();
        for (index, instance) in instances.into_iter().enumerate() {
            let first = match read_resources(&instance) {
                Ok(first) => first,
                Err(error) if index != 0 => {
                    warnings.push(format!(
                        "pre-work resource process {}: {error}",
                        instance.pid
                    ));
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let target_id = if index == 0 {
                format!("target-{prefix}")
            } else {
                format!("target-{prefix}-{}", instance.pid)
            };
            let target = project.register_target(
                run,
                &target_id,
                serde_json::to_value(&instance)?,
                backend.owns_resources(),
            )?;
            let source_id = if index == 0 {
                format!("resources-{prefix}")
            } else {
                format!("resources-{prefix}-{}", instance.pid)
            };
            let source = project.register_source(
                run,
                hunin_platforms::recording::resource_definition(&source_id, target.clone(), &first),
            )?;
            preparation.sources.push(source.clone());
            source.append(&[first.to_record("0", &target.id)?])?;
            source.ready(json!({"first_acquisition":first.acquired,"scope":"one process instance, all threads, excluding children"}))?;
            resources.push(ProcessResources::new(target, instance, source, first));
        }
        let target = resources
            .first()
            .ok_or(Error::Invalid("capture has no readable primary target"))?
            .target
            .clone();
        let mut definition = backend.definition(&format!("code-{prefix}"), &target);
        let metadata = definition
            .metadata
            .as_object_mut()
            .ok_or(Error::Invalid("source metadata must be an object"))?;
        metadata.insert("preparation_artifacts".into(),json!({"sha256":hashes,"meaning":"input snapshots before preparation and target launch"}));
        metadata.insert("resource_population".into(), population);
        let code = project.register_source(run, definition)?;
        preparation.sources.push(code.clone());
        code.ready(json!({"basis":"prepared observer acknowledged source start before work release","scope":"declared runtime or browser collection; child stream schemas can become available at stop"}))?;
        preparation.published = true;
        Ok(Capture {
            project,
            controller: controller.clone(),
            request: request.clone(),
            generation,
            target,
            process: preparation.process.take().expect("prepared process"),
            backend,
            area: preparation.area.take(),
            resources,
            code,
            derived_sources: Vec::new(),
            population_complete: warnings.is_empty(),
            warnings,
            released: false,
            finalized: false,
            cleanup_confirmed: false,
            import_complete: false,
            retained_area: None,
        })
    })();
    if let Err(error) = &result {
        preparation.reason = error.to_string();
    }
    result
}

struct CaptureFailure<'a> {
    reason: &'a str,
    input: Option<&'a Path>,
}

fn cleanup_area(
    area: &mut Option<tempfile::TempDir>,
    retained: &mut Option<PathBuf>,
    keep: bool,
) -> std::io::Result<()> {
    if let Some(area) = area.take() {
        *retained = Some(area.keep());
    }
    if !keep && let Some(path) = retained.as_ref() {
        match std::fs::remove_dir_all(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        *retained = None;
    }
    Ok(())
}

fn abort_capture(
    controller: &Controller,
    request: &RequestId,
    generation: i64,
    process: Option<&mut PreparedProcess>,
    sources: &[SourceHandle],
    temporary: (&mut Option<tempfile::TempDir>, &mut Option<PathBuf>),
    failure: CaptureFailure<'_>,
) -> Result<RequestOutcome, Error> {
    let (area, retained_area) = temporary;
    let mut pending = false;
    if let Some(process) = process {
        let cancelled = process.cancel(Duration::from_millis(100), Duration::from_secs(2));
        pending = cancelled
            .as_ref()
            .map_or(true, |outcome| outcome.cleanup_pending);
        if pending {
            pending = !process.confirm_cleanup(Duration::ZERO).unwrap_or(false);
        }
    }
    let reason = failure.reason;
    let retain_inputs = failure.input.is_some_and(|path| {
        std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
    });
    let retained_for_retry = retain_inputs
        .then(|| {
            area.as_ref()
                .map(|area| area.path().to_owned())
                .or_else(|| retained_area.clone())
        })
        .flatten();
    let mut failures = Vec::new();
    let source_outcome = json!({"complete":false,"reason":reason,"cleanup_pending":pending,"durability_boundary":"previously committed structured records; no claim for unimported producer output","temporary_area":retained_for_retry,"source_inputs_retained_for_retry":retain_inputs});
    for source in sources {
        if source.status().is_ok_and(|status| status.closed) {
            continue;
        }
        if let Err(error) = source.close(source_outcome.clone()) {
            failures.push(error.to_string());
        }
    }
    pending |= !failures.is_empty();
    if let Err(error) = cleanup_area(area, retained_area, pending || retain_inputs) {
        pending = true;
        failures.push(error.to_string());
    }
    if pending {
        controller.interrupted(request)?;
    }
    let result = json!({"error":reason,"application_outcome":"unknown","collection_complete":false,"cleanup_pending":pending,"temporary_area":retained_area,"source_inputs_retained_for_retry":retain_inputs,"cleanup_errors":failures});
    let outcome = controller.complete(request, generation, false, result.clone())?;
    if outcome.state == "uncertain" && !pending {
        return Ok(controller.reconcile(request, result, true)?);
    }
    Ok(outcome)
}

impl Capture {
    pub fn project(&self) -> &Project {
        &self.project
    }
    pub fn target(&self) -> &TargetRef {
        &self.target
    }
    pub fn resource_source(&self) -> &SourceHandle {
        &self.resources[0].source
    }
    pub fn resource_sources(&self) -> impl Iterator<Item = &SourceHandle> {
        self.resources.iter().map(|resource| &resource.source)
    }
    pub fn collection_source(&self) -> &SourceHandle {
        &self.code
    }
    fn owned_sources(&self) -> Vec<SourceHandle> {
        self.resources
            .iter()
            .map(|resource| resource.source.clone())
            .chain(std::iter::once(self.code.clone()))
            .chain(self.derived_sources.iter().cloned())
            .collect()
    }
    pub fn release(&mut self) -> Result<(), Error> {
        let request = self.controller.lookup(&self.request)?;
        let state = self.controller.run_state(&self.code.run())?;
        if self.finalized
            || request.state != "dispatched"
            || request.generation != self.generation
            || !state["terminal"].is_null()
            || state["generation"] != self.generation
        {
            return Err(hunin_core::project::Error::Conflict(
                "capture no longer has an active launch generation".into(),
            )
            .into());
        }
        for source in self.owned_sources() {
            let status = source.status()?;
            if !status.ready || status.closed {
                return Err(Error::Invalid(
                    "work requires open ready observation sources",
                ));
            }
        }
        self.process.release()?;
        self.released = true;
        Ok(())
    }
    pub fn sample(&mut self) -> Result<(), Error> {
        for resource in &mut self.resources {
            if let Some(warning) = resource.sample()? {
                self.warnings.push(warning);
            }
        }
        Ok(())
    }
    pub fn finish(mut self, timeout: Duration) -> Result<CaptureReport, Error> {
        let result = self.finish_inner(timeout);
        if let Err(error) = &result {
            let sources = self.owned_sources();
            let _ = abort_capture(
                &self.controller,
                &self.request,
                self.generation,
                (!self.cleanup_confirmed).then_some(&mut self.process),
                &sources,
                (&mut self.area, &mut self.retained_area),
                CaptureFailure {
                    reason: &error.to_string(),
                    input: (!self.import_complete).then_some(self.backend.profile_path()),
                },
            );
            self.finalized = true;
        }
        result
    }
    pub fn cancel(&mut self, request: RequestId) -> Result<RequestOutcome, Error> {
        let accepted = self.controller.accept(
            &request,
            &self.code.run(),
            Action::Cancel,
            json!({"capture_request":self.request,"owned_scope":"prepared process group or job"}),
        )?;
        let dispatched = if accepted.newly_accepted || accepted.outcome.state == "accepted" {
            self.controller.dispatch(&request)?
        } else if matches!(accepted.outcome.state.as_str(), "dispatched" | "uncertain") {
            accepted.outcome
        } else {
            return Ok(accepted.outcome);
        };
        let result = (|| {
            let cleanup = self
                .process
                .cancel(Duration::from_millis(100), Duration::from_secs(2));
            let mut pending = !self
                .process
                .confirm_cleanup(Duration::ZERO)
                .unwrap_or(false);
            self.cleanup_confirmed = !pending;
            let retain_inputs = !self.import_complete
                && std::fs::symlink_metadata(self.backend.profile_path())
                    .is_ok_and(|metadata| metadata.file_type().is_file());
            let area_path = self
                .area
                .as_ref()
                .map(|area| area.path().to_owned())
                .or_else(|| self.retained_area.clone());
            let outcome = json!({"complete":false,"reason":"caller cancelled capture","durability_boundary":"previously committed structured records only","cleanup_pending":pending,"temporary_area":area_path,"source_inputs_retained_for_retry":retain_inputs});
            for source in self.owned_sources() {
                if !source.status()?.closed {
                    source.close(outcome.clone())?;
                }
            }
            let mut cleanup_errors = Vec::new();
            if let Err(error) = cleanup_area(
                &mut self.area,
                &mut self.retained_area,
                pending || retain_inputs,
            ) {
                pending = true;
                cleanup_errors.push(error.to_string());
            }
            if pending {
                self.controller.interrupted(&request)?;
            }
            let result = json!({"cancelled":true,"cleanup_pending":pending,"temporary_area":self.retained_area,"source_inputs_retained_for_retry":retain_inputs,"cleanup_errors":cleanup_errors,"process_error":cleanup.err().map(|error|error.to_string())});
            let mut outcome = self.controller.complete(
                &request,
                dispatched.generation,
                !pending,
                result.clone(),
            )?;
            if outcome.state == "uncertain" && !pending {
                outcome = self.controller.reconcile(&request, result, true)?;
            }
            if self.controller.lookup(&self.request)?.state == "uncertain" {
                self.controller.reconcile(&self.request,json!({"cancel_request":request,"target":self.target,"temporary_area":self.retained_area}),!pending)?;
            }
            self.finalized = true;
            Ok(outcome)
        })();
        if result.is_err() {
            let _ = self.controller.interrupted(&request);
        }
        result
    }
    fn finish_inner(&mut self, timeout: Duration) -> Result<CaptureReport, Error> {
        if !self.released {
            return Err(Error::Invalid("capture has not released the workload"));
        }
        let until = Instant::now()
            .checked_add(timeout)
            .ok_or(Error::Invalid("invalid capture deadline"))?;
        loop {
            if self.process.work_complete()? {
                break;
            }
            if Instant::now() >= until {
                return Err(Error::Invalid("work completion deadline exceeded"));
            }
            if std::fs::metadata(self.backend.profile_path())
                .is_ok_and(|metadata| metadata.len() > 16 * 1024 * 1024)
            {
                return Err(Error::Invalid(
                    "runtime recording exceeds extraction input budget",
                ));
            }
            self.sample()?;
            std::thread::sleep(
                Duration::from_millis(50).min(until.saturating_duration_since(Instant::now())),
            );
        }
        self.sample()?;
        self.process.finish()?;
        let execution = self
            .process
            .wait(Duration::from_secs(5).min(until.saturating_duration_since(Instant::now())))?;
        self.cleanup_confirmed = !execution.cleanup_pending;
        let subject = if self.backend.is_browser() {
            format!("collection:{}", self.code.id())
        } else {
            self.target.id.clone()
        };
        if Instant::now() >= until {
            return Err(Error::Invalid(
                "capture deadline exceeded before extraction",
            ));
        }
        let extracted = self.backend.extract(&subject)?;
        let collection = extracted.collection;
        let application_record = collection
            .records
            .iter()
            .find(|record| record.schema == "prepared.report")
            .ok_or(Error::Invalid("missing application report"))?;
        let application = application_record.payload.clone();
        let application_reference = self.code.record_ref(&application_record.id);
        let mut code_records = publish(&self.code, &collection, until)?;
        let mut code_sources = Vec::new();
        let mut capture_complete = collection.capture_complete;
        self.warnings.extend(collection.warnings.clone());
        if !self.backend.is_browser() {
            code_sources.push(self.code.id().to_owned());
        }
        for stream in extracted.streams {
            let target = stream.process_id.and_then(|pid| {
                self.resources
                    .iter()
                    .find(|resource| {
                        resource.instance.pid == pid && resource.final_instance_verified
                    })
                    .map(|resource| &resource.target)
            });
            let stream_id = format!(
                "{}-stream-{:x}",
                self.code.id(),
                Sha256::digest(stream.key.as_bytes())
            );
            let mut definition = stream.definition(&stream_id, target);
            let metadata = definition
                .metadata
                .as_object_mut()
                .ok_or(Error::Invalid("browser stream metadata must be an object"))?;
            metadata.insert("parent_collection".into(),json!({"project":self.project.id(),"run":self.code.run().id,"source":self.code.id()}));
            metadata.insert("reported_process_id".into(), json!(stream.process_id));
            metadata.insert(
                "process_binding".into(),
                json!(if target.is_some() {
                    "matched continuously validated pre-work process instance"
                } else {
                    "unavailable; no process or page CPU ownership inferred"
                }),
            );
            let source = self.project.register_source(&self.code.run(), definition)?;
            self.derived_sources.push(source.clone());
            source.ready(json!({"basis":"exported stream schema validated before ingestion","capture_readiness_source":self.code.id(),"schema_available":"at source stop; no synthetic start timestamp"}))?;
            code_records += publish(&source, &stream.extraction, until)?;
            capture_complete &= stream.extraction.capture_complete;
            self.warnings.extend(stream.extraction.warnings.clone());
            source.close(json!({"supported_retention_complete":stream.extraction.capture_complete,"warnings":stream.extraction.warnings,"extraction_metadata":stream.extraction.metadata,"parent_collection_source":self.code.id()}))?;
            code_sources.push(source.id().to_owned());
        }
        let application_success = execution.success && application["outcome"] == "completed";
        self.code.close(json!({"supported_retention_complete":capture_complete,"warnings":self.warnings,"extraction_metadata":collection.metadata,"application_success":application_success,"cleanup_pending":execution.cleanup_pending,"code_sources":code_sources}))?;
        for resource in &self.resources {
            resource.close()?;
        }
        self.import_complete = true;
        let collection_complete = capture_complete
            && self.population_complete
            && self.resources.iter().all(|resource| resource.complete)
            && self.cleanup_confirmed;
        let cpu_ns = self.resources.iter().try_fold(0_u64, |sum, resource| {
            sum.checked_add(resource.cpu_ns()?)
                .ok_or(Error::Invalid("process population cpu sum overflow"))
        })?;
        let elapsed = execution
            .measured_elapsed
            .unwrap_or(execution.elapsed)
            .as_nanos()
            .to_string();
        let targets = self
            .resources
            .iter()
            .map(|resource| resource.target.clone())
            .collect::<Vec<_>>();
        let resource_sources = self
            .resources
            .iter()
            .map(|resource| resource.source.id().to_owned())
            .collect::<Vec<_>>();
        cleanup_area(
            &mut self.area,
            &mut self.retained_area,
            !self.cleanup_confirmed,
        )?;
        if execution.cleanup_pending {
            self.controller.interrupted(&self.request)?;
        }
        let outcome=self.controller.complete(&self.request,self.generation,application_success&&collection_complete,json!({"application_success":application_success,"application_report":application_reference,"collection_complete":collection_complete,"target":self.target,"targets":targets,"collection_source":self.code.id(),"resource_sources":resource_sources,"code_sources":code_sources,"cpu_ns":cpu_ns.to_string(),"cpu_scope":"sum of explicit pre-work process instances, all threads excluding children; no implicit operation/page ownership","elapsed_ns":elapsed,"elapsed_scope":"controller release to helper completion acknowledgment, including producer drain/export and excluding final held exit","cleanup_pending":execution.cleanup_pending,"temporary_area":self.retained_area,"source_inputs_retained_for_retry":false}))?;
        self.finalized = true;
        Ok(CaptureReport {
            request: outcome,
            target: self.target.clone(),
            targets,
            collection_source: self.code.id().into(),
            resource_sources,
            code_sources,
            application,
            elapsed_ns: elapsed,
            cpu_ns: cpu_ns.to_string(),
            collection_complete,
            application_success,
            resource_observations: self
                .resources
                .iter()
                .map(|resource| resource.snapshots)
                .sum(),
            code_records,
            warnings: self.warnings.clone(),
        })
    }
}

fn publish(
    source: &SourceHandle,
    extracted: &hunin_runtimes::Extraction,
    deadline: Instant,
) -> Result<usize, Error> {
    let mut batch = Vec::new();
    let mut bytes = 0;
    let mut delivery = 0;
    for record in &extracted.records {
        if Instant::now() >= deadline {
            return Err(Error::Invalid(
                "source ingestion deadline exceeded; committed prefix remains durable",
            ));
        }
        let size = serde_json::to_vec(record)?.len();
        if !batch.is_empty() && (batch.len() == 256 || bytes + size > 4 * 1024 * 1024) {
            source.append_batch(&format!("extraction-{delivery}"), &batch)?;
            batch.clear();
            bytes = 0;
            delivery += 1;
        }
        batch.push(record.clone());
        bytes += size;
    }
    if !batch.is_empty() {
        source.append_batch(&format!("extraction-{delivery}"), &batch)?;
    }
    Ok(extracted.records.len())
}

impl Drop for Capture {
    fn drop(&mut self) {
        if !self.finalized {
            let sources = self.owned_sources();
            let _ = abort_capture(
                &self.controller,
                &self.request,
                self.generation,
                (!self.cleanup_confirmed).then_some(&mut self.process),
                &sources,
                (&mut self.area, &mut self.retained_area),
                CaptureFailure {
                    reason: "capture ended without finalization",
                    input: (!self.import_complete).then_some(self.backend.profile_path()),
                },
            );
        }
    }
}

use crate::{CapturePlan, Error};
use hunin_core::{
    control::RequestId,
    execution::PreparedLaunch,
    project::{Reference, TargetRef},
    records::SourceDefinition,
};
use hunin_platforms::{ProcessInstance, discover_processes, inspect_process};
use hunin_runtimes::{
    Extraction, Limits, PreparedRuntime, RuntimeKind,
    browser::{self, BrowserKind, BrowserStream, PreparedBrowser},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub struct BrowserCapturePlan {
    pub request: RequestId,
    pub run: Reference,
    pub kind: BrowserKind,
    pub browser: PathBuf,
    pub node: PathBuf,
    pub driver: Option<PathBuf>,
    pub driver_script: Option<PathBuf>,
    pub workload: PathBuf,
    pub conditions: Value,
    pub environment: BTreeMap<String, String>,
    pub read_paths: Vec<PathBuf>,
    pub write_paths: Vec<PathBuf>,
    pub safari_scripts: Vec<PathBuf>,
    pub babel_root: Option<PathBuf>,
    pub babel_node: Option<PathBuf>,
}

pub(crate) enum Backend {
    Runtime(PreparedRuntime),
    Browser(PreparedBrowser),
}
pub(crate) struct Extracted {
    pub collection: Extraction,
    pub streams: Vec<BrowserStream>,
}

impl Backend {
    pub fn runtime(plan: &CapturePlan, area: &Path) -> Result<Self, Error> {
        let mut runtime = if plan.runtime == RuntimeKind::PHP {
            hunin_runtimes::php::prepare(
                &plan.executable,
                plan.xdebug_extension.as_deref().ok_or(Error::Invalid(
                    "PHP capture requires its explicit Xdebug extension path",
                ))?,
                &plan.workload,
                area,
            )?
        } else {
            if plan.xdebug_extension.is_some() {
                return Err(Error::Invalid("Xdebug configuration only applies to PHP"));
            }
            hunin_runtimes::prepare(plan.runtime, &plan.executable, &plan.workload, area)?
        };
        runtime.configure(&plan.environment, &plan.read_paths, &plan.write_paths)?;
        runtime
            .launch
            .environment
            .insert("HUNIN_FINALIZE_GATE".into(), "1".into());
        Ok(Self::Runtime(runtime))
    }
    pub fn browser(plan: &BrowserCapturePlan, area: &Path) -> Result<Self, Error> {
        let mut browser = browser::prepare(
            plan.kind,
            &plan.browser,
            &plan.node,
            plan.driver.as_deref(),
            &plan.workload,
            area,
        )?;
        browser.configure(&plan.environment, &plan.read_paths, &plan.write_paths)?;
        if let Some(driver) = &plan.driver_script {
            browser.configure_driver(driver)?;
        }
        if plan.kind == BrowserKind::Safari {
            let compiler = plan.babel_node.as_deref().ok_or(Error::Invalid(
                "Safari capture requires its explicit compiler Node executable",
            ))?;
            let babel = plan.babel_root.as_deref().ok_or(Error::Invalid(
                "Safari capture requires its explicit Babel root",
            ))?;
            if plan.safari_scripts.is_empty() {
                return Err(Error::Invalid(
                    "Safari capture requires an explicit script list",
                ));
            }
            for script in &plan.safari_scripts {
                browser.instrument_safari_script(script, compiler, babel)?;
            }
        } else if !plan.safari_scripts.is_empty()
            || plan.babel_root.is_some()
            || plan.babel_node.is_some()
        {
            return Err(Error::Invalid(
                "Safari instrumentation configuration only applies to Safari",
            ));
        }
        Ok(Self::Browser(browser))
    }
    pub fn launch(&self) -> &PreparedLaunch {
        match self {
            Self::Runtime(source) => &source.launch,
            Self::Browser(source) => &source.launch,
        }
    }
    pub fn profile_path(&self) -> &Path {
        match self {
            Self::Runtime(source) => &source.profile_path,
            Self::Browser(source) => &source.profile_path,
        }
    }
    pub fn definition(&self, id: &str, target: &TargetRef) -> SourceDefinition {
        match self {
            Self::Runtime(source) => source.definition(id, target),
            Self::Browser(source) => source.definition(id, target),
        }
    }
    pub fn extract(&self, subject: &str) -> Result<Extracted, Error> {
        Ok(match self {
            Self::Runtime(source) => Extracted {
                collection: source.extract(subject, Limits::default())?,
                streams: vec![],
            },
            Self::Browser(source) => {
                let extracted = source.extract(subject, Limits::default())?;
                Extracted {
                    collection: extracted.collection,
                    streams: extracted.streams,
                }
            }
        })
    }
    pub fn is_browser(&self) -> bool {
        matches!(self, Self::Browser(_))
    }
    pub fn owns_resources(&self) -> bool {
        !matches!(self,Self::Browser(source)if source.kind==BrowserKind::Safari)
    }
    pub fn instances(
        &self,
        helper: u32,
    ) -> Result<(Vec<ProcessInstance>, Vec<String>, Value), Error> {
        let Self::Browser(browser) = self else {
            return Ok((
                vec![inspect_process(helper)?],
                vec![],
                json!({"scope":"one prepared runtime process"}),
            ));
        };
        let targets = browser.targets()?;
        if targets.pid == 0 || targets.pid == helper {
            return Err(Error::Invalid(
                "browser target must be the actual browser process, not its observer helper",
            ));
        }
        let primary = inspect_process(targets.pid)?;
        let mut selected = BTreeSet::from([targets.pid]);
        let mut warnings = Vec::new();
        for process in &targets.processes {
            if let Some(pid) = process
                .get("id")
                .or_else(|| process.get("pid"))
                .and_then(Value::as_u64)
            {
                let pid = u32::try_from(pid)
                    .map_err(|_| Error::Invalid("reported browser pid exceeds platform range"))?;
                if pid != helper && pid != 0 {
                    selected.insert(pid);
                }
            }
        }
        let mut discovered = BTreeMap::new();
        match discover_processes() {
            Ok(result) => {
                for instance in result.processes {
                    discovered.insert(instance.pid, instance);
                }
                loop {
                    let before = selected.len();
                    for instance in discovered.values() {
                        if instance
                            .parent_pid
                            .is_some_and(|parent| selected.contains(&parent))
                        {
                            selected.insert(instance.pid);
                        }
                    }
                    if before == selected.len() {
                        break;
                    }
                }
            }
            Err(error) => {
                warnings.push(format!("browser descendant discovery unavailable: {error}"))
            }
        }
        if selected.len() > 128 {
            return Err(Error::Invalid(
                "browser pre-work resource population exceeds 128 processes",
            ));
        }
        let mut instances = vec![primary];
        for pid in selected.into_iter().filter(|pid| *pid != targets.pid) {
            match inspect_process(pid) {
                Ok(instance) => instances.push(instance),
                Err(error) => warnings.push(format!(
                    "reported pre-work process {pid} unavailable: {error}"
                )),
            }
        }
        let metadata = json!({"reported_targets":targets,"resource_scope":"explicit pre-work process instances from browser protocol and OS descendants; not page-owned cpu","unavailable":warnings});
        Ok((instances, warnings, metadata))
    }
}

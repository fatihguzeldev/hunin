mod cdp;
mod gecko;
pub mod maps;
pub mod safari;

use crate::{Error, Extraction, Limits, Result, record, schema, v8};
use hunin_core::{
    execution::PreparedLaunch,
    project::TargetRef,
    records::{FieldType, SourceDefinition},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserKind {
    Chromium,
    Firefox,
    Safari,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowserTargets {
    pub pid: u32,
    pub processes: Vec<Value>,
    pub targets: Vec<Value>,
    pub version: Value,
}

pub struct PreparedBrowser {
    pub kind: BrowserKind,
    pub launch: PreparedLaunch,
    pub profile_path: PathBuf,
    pub report_path: PathBuf,
    pub targets_path: PathBuf,
    safari_scripts: Vec<Value>,
}

pub struct BrowserStream {
    pub key: String,
    pub process_id: Option<u32>,
    pub extraction: Extraction,
    interpretation: &'static str,
}
pub struct BrowserExtraction {
    pub collection: Extraction,
    pub streams: Vec<BrowserStream>,
}

impl BrowserStream {
    pub fn definition(&self, id: &str, target: Option<&TargetRef>) -> SourceDefinition {
        SourceDefinition {
            id: id.into(),
            target: target.cloned(),
            schemas: self.extraction.schemas.clone(),
            clocks: self.extraction.clocks.clone(),
            interpretation: self.interpretation.into(),
            metadata: self.extraction.metadata.clone(),
        }
    }
}

pub fn prepare(
    kind: BrowserKind,
    browser: &Path,
    node: &Path,
    driver: Option<&Path>,
    workload: &Path,
    area: &Path,
) -> Result<PreparedBrowser> {
    if ![browser, node, workload, area]
        .iter()
        .all(|p| p.is_absolute())
    {
        return Err(Error("browser preparation needs absolute paths".into()));
    }
    if !fs::symlink_metadata(area)
        .map_err(|e| Error(e.to_string()))?
        .file_type()
        .is_dir()
    {
        return Err(Error(
            "browser area must be an owned directory, not a symlink".into(),
        ));
    }
    let area = fs::canonicalize(area).map_err(|e| Error(e.to_string()))?;
    let workload = fs::canonicalize(workload).map_err(|e| Error(e.to_string()))?;
    let browser = fs::canonicalize(browser).map_err(|e| Error(e.to_string()))?;
    let node = fs::canonicalize(node).map_err(|e| Error(e.to_string()))?;
    if kind == BrowserKind::Firefox && driver.is_none() {
        return Err(Error(
            "firefox requires an explicit matching geckodriver".into(),
        ));
    }
    if kind == BrowserKind::Safari && (!cfg!(target_os = "macos") || driver.is_none()) {
        return Err(Error(
            "Safari requires macOS and the explicit bundled safaridriver".into(),
        ));
    }
    let helper = area.join("browser.cjs");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&helper)
        .map_err(|e| Error(e.to_string()))?;
    file.write_all(include_str!("../../helpers/browser.cjs").as_bytes())
        .map_err(|e| Error(e.to_string()))?;
    let kind_name = match kind {
        BrowserKind::Chromium => "chromium",
        BrowserKind::Firefox => "firefox",
        BrowserKind::Safari => "safari",
    };
    let driver = driver
        .map(fs::canonicalize)
        .transpose()
        .map_err(|e| Error(e.to_string()))?
        .unwrap_or_default();
    Ok(PreparedBrowser {
        kind,
        launch: PreparedLaunch {
            program: node,
            args: vec![
                helper.to_string_lossy().into_owned(),
                kind_name.into(),
                browser.to_string_lossy().into_owned(),
                driver.to_string_lossy().into_owned(),
                workload.to_string_lossy().into_owned(),
                area.to_string_lossy().into_owned(),
            ],
            directory: area.clone(),
            environment: BTreeMap::from([
                ("TMPDIR".into(), area.to_string_lossy().into_owned()),
                ("TMP".into(), area.to_string_lossy().into_owned()),
                ("TEMP".into(), area.to_string_lossy().into_owned()),
                ("HUNIN_FINALIZE_GATE".into(), "1".into()),
            ]),
            stdout_limit: 64 * 1024,
        },
        profile_path: area.join("browser-profile.json"),
        report_path: area.join("browser-report.json"),
        targets_path: area.join("browser-targets.json"),
        safari_scripts: vec![],
    })
}

fn collection_schemas() -> Vec<hunin_core::records::RecordSchema> {
    let mut schemas = vec![
        schema(
            "browser.collection",
            "browser collection identity, scope and exported stream bindings",
            &[
                (
                    "kind",
                    FieldType::String,
                    None,
                    "actual browser source kind",
                ),
                (
                    "metadata",
                    FieldType::Object,
                    None,
                    "source configuration, browser identity and scope",
                ),
                (
                    "coverage",
                    FieldType::Object,
                    None,
                    "source roster/export gaps and interpretation; not page ownership",
                ),
            ],
        ),
        schema(
            "prepared.report",
            "prepared browser application outcome, separate from collection success",
            &[(
                "outcome",
                FieldType::String,
                None,
                "application completion status",
            )],
        ),
        schema(
            "browser.script",
            "executed script identity from the browser debugger",
            &[
                (
                    "script_id",
                    FieldType::String,
                    None,
                    "browser source-local script id",
                ),
                (
                    "sha256",
                    FieldType::String,
                    None,
                    "sha256 of exact debugger-returned script bytes",
                ),
                (
                    "metadata",
                    FieldType::Object,
                    None,
                    "script url, session, range and optional producer fields",
                ),
            ],
        ),
        schema(
            "browser.script-chunk",
            "bounded exact UTF-8 executed script content",
            &[
                (
                    "script_id",
                    FieldType::String,
                    None,
                    "script descriptor identity",
                ),
                ("index", FieldType::Unsigned, None, "ordered content chunk"),
                (
                    "text",
                    FieldType::String,
                    None,
                    "actual debugger-returned source text",
                ),
            ],
        ),
    ];
    for (name, meaning) in [
        (
            "browser.target",
            "declared profiler roster, reported targets and identity-only observers; role is explicit",
        ),
        (
            "browser.execution-context",
            "Runtime.executionContextCreated identity including source session and raw frame binding",
        ),
        (
            "browser.frame",
            "raw Page.getFrameTree frame and its source target/session",
        ),
        (
            "browser.browsing-context",
            "raw BiDi browsing context and selection metadata",
        ),
        (
            "browser.realm",
            "raw BiDi realm identity, worker ownership and context bindings",
        ),
    ] {
        schemas.push(schema(name, meaning, &[("role", FieldType::String, None, "source identity collection role"),("binding",FieldType::Object,None,"source-specific identity fields preserved without inferred operation ownership")]));
    }
    schemas
}

impl PreparedBrowser {
    pub fn instrument_safari_script(
        &mut self,
        source: &Path,
        compiler_node: &Path,
        babel_root: &Path,
    ) -> Result<()> {
        if self.kind != BrowserKind::Safari || self.safari_scripts.len() >= 16 {
            return Err(Error(
                "Safari instrumentation requires a Safari collector and at most16 scripts".into(),
            ));
        }
        let source = fs::canonicalize(source).map_err(|e| Error(e.to_string()))?;
        let index = self.safari_scripts.len();
        let area = &self.launch.directory;
        let output = area.join(format!("safari-script-{index}.js"));
        let binding = area.join(format!("safari-script-{index}.json"));
        safari::instrument(compiler_node, babel_root, &source, &output, &binding, area)?;
        self.safari_scripts
            .push(json!({"original":source,"instrumented":output,"binding":binding}));
        let config = area.join("safari-scripts.json");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&config)
            .map_err(|e| Error(e.to_string()))?;
        file.write_all(
            &serde_json::to_vec(&self.safari_scripts).map_err(|e| Error(e.to_string()))?,
        )
        .map_err(|e| Error(e.to_string()))?;
        file.sync_all().map_err(|e| Error(e.to_string()))?;
        self.launch.environment.insert(
            "HUNIN_SAFARI_SCRIPTS".into(),
            config.to_string_lossy().into_owned(),
        );
        Ok(())
    }
    /// explicit target environment and declared filesystem inputs/outputs, not a sandbox.
    pub fn configure(
        &mut self,
        environment: &BTreeMap<String, String>,
        read_paths: &[PathBuf],
        write_paths: &[PathBuf],
    ) -> Result<()> {
        for (key, value) in environment {
            if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
                return Err(Error("invalid browser environment key/value".into()));
            }
            if key.to_ascii_uppercase().starts_with("HUNIN_")
                || self.launch.environment.keys().any(|owned| {
                    if cfg!(windows) {
                        owned.eq_ignore_ascii_case(key)
                    } else {
                        owned == key
                    }
                })
            {
                return Err(Error(format!(
                    "browser environment {key} is owned by its collector"
                )));
            }
        }
        let paths = read_paths.iter().chain(write_paths);
        for path in paths {
            if !path.is_absolute() {
                return Err(Error("browser filesystem paths must be absolute".into()));
            }
            fs::canonicalize(path).map_err(|e| Error(e.to_string()))?;
        }
        self.launch.environment.extend(environment.clone());
        Ok(())
    }
    /// an agent-prepared module may supply page input and consume page output.
    pub fn configure_driver(&mut self, path: &Path) -> Result<()> {
        if !path.is_absolute() {
            return Err(Error("browser driver path must be absolute".into()));
        }
        let path = fs::canonicalize(path).map_err(|e| Error(e.to_string()))?;
        let bytes = read_owned(&path, 1024 * 1024)?;
        self.launch.environment.insert(
            "HUNIN_BROWSER_DRIVER".into(),
            path.to_string_lossy().into_owned(),
        );
        self.launch.environment.insert(
            "HUNIN_BROWSER_DRIVER_SHA256".into(),
            crate::source_maps::digest(&bytes),
        );
        Ok(())
    }
    pub fn targets(&self) -> Result<BrowserTargets> {
        serde_json::from_slice(&read_owned(&self.targets_path, 1024 * 1024)?)
            .map_err(|e| Error(e.to_string()))
    }
    pub fn definition(&self, id: &str, target: &TargetRef) -> SourceDefinition {
        SourceDefinition {
            id: id.into(),
            target: Some(target.clone()),
            schemas: collection_schemas(),
            clocks: vec![],
            interpretation: "browser-collection-v2".into(),
            metadata: json!({"method":if self.kind==BrowserKind::Safari {"compiler-inserted function observation"}else{"browser profiler"},"kind":format!("{:?}",self.kind),"stream_registration":"source-specific streams are declared before durable extraction; data may become available only at source stop"}),
        }
    }
    pub fn extract(&self, subject: &str, limits: Limits) -> Result<BrowserExtraction> {
        let bytes = read_owned(&self.profile_path, limits.max_input_bytes)?;
        let body: Value = serde_json::from_slice(&bytes).map_err(|e| Error(e.to_string()))?;
        extract(
            &body,
            subject,
            limits,
            Some(
                serde_json::from_slice(&read_owned(&self.report_path, limits.max_input_bytes)?)
                    .map_err(|e| Error(e.to_string()))?,
            ),
        )
    }
}

fn identity_records(out: &mut Extraction, body: &Value, subject: &str) -> Result<()> {
    for (field, role) in [
        ("targets", "profiler-roster"),
        ("reportedTargets", "reported-target"),
        ("contextObservers", "context-observer"),
    ] {
        for (index, binding) in body[field].as_array().into_iter().flatten().enumerate() {
            out.push(record(
                format!("identity:{field}:{index}"),
                "browser.target",
                subject,
                json!({"role":role,"binding":binding}),
            ))?;
        }
    }
    for (index, binding) in body["executionContexts"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        out.push(record(
            format!("identity:context:{index}"),
            "browser.execution-context",
            subject,
            json!({"role":"execution-context-created","binding":binding}),
        ))?;
    }
    for (tree_index, tree) in body["frameTrees"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let mut pending = vec![(&tree["frameTree"], 0usize)];
        let mut index = 0;
        while let Some((node, depth)) = pending.pop() {
            if depth > 32 {
                return Err(Error("browser frame tree exceeds depth budget".into()));
            }
            out.push(record(format!("identity:frame:{tree_index}:{index}"),"browser.frame",subject,json!({"role":"page-frame-tree","binding":{"frame":node["frame"],"targetId":tree["targetId"],"sessionId":tree["sessionId"]}})))?;
            index += 1;
            pending.extend(
                node["childFrames"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|child| (child, depth + 1)),
            );
        }
    }
    for (index, binding) in body["scopeBindings"]["contexts"]["contexts"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        out.push(record(format!("identity:browsing-context:{index}"),"browser.browsing-context",subject,json!({"role":"bidi-context-tree","binding":binding,"selected":binding["context"]==body["scopeBindings"]["selectedContext"]})))?;
    }
    for (index, binding) in body["scopeBindings"]["realms"]["realms"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        out.push(record(
            format!("identity:realm:{index}"),
            "browser.realm",
            subject,
            json!({"role":"bidi-realm","binding":binding}),
        ))?;
    }
    Ok(())
}

fn coverage_warnings(body: &Value, kind: &str) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    if kind == "chromium" {
        let Some(targets) = body["targets"].as_array() else {
            return Ok(vec![
                "target readiness roster unavailable; worker coverage is unverified".into(),
            ]);
        };
        if targets.len() > 128 {
            return Err(Error("browser target roster exceeds budget".into()));
        }
        let mut seen = std::collections::BTreeSet::new();
        for target in targets {
            let id = target["targetId"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 128)
                .ok_or_else(|| Error("invalid declared target id".into()))?;
            if !seen.insert(id) {
                return Err(Error("duplicate declared target identity".into()));
            }
            let matching = body["profiles"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|profile| {
                    profile["targetId"] == id && profile["sessionId"] == target["sessionId"]
                });
            if target["ready"] != true || matching.is_none() {
                warnings.push(format!(
                    "declared {} target {id} did not provide an acknowledged matching profile",
                    target["type"].as_str().unwrap_or("unknown")
                ));
            }
        }
        for profile in body["profiles"].as_array().into_iter().flatten() {
            if !profile["targetId"]
                .as_str()
                .is_some_and(|id| seen.contains(id))
            {
                warnings.push("exported profile has no declared target binding".into());
            }
        }
    } else if kind == "firefox" {
        let Some(realms) = body["scopeBindings"]["realms"]["realms"].as_array() else {
            return Ok(vec![
                "realm roster unavailable; global-preset worker coverage is unverified".into(),
            ]);
        };
        let mut pending = vec![(&body["profile"], 0usize)];
        let mut thread_sources = Vec::new();
        let mut processes = 0;
        while let Some((profile, depth)) = pending.pop() {
            processes += 1;
            if depth > 16 || processes > 128 {
                return Err(Error("gecko process graph exceeds budget".into()));
            }
            for thread in profile["threads"].as_array().into_iter().flatten() {
                thread_sources.push(thread);
            }
            pending.extend(
                profile["processes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|child| (child, depth + 1)),
            );
        }
        for realm in realms
            .iter()
            .filter(|realm| realm["type"] == "dedicated-worker")
        {
            let origin = realm["origin"].as_str().filter(|origin| !origin.is_empty());
            let observed = origin.is_some_and(|origin| {
                thread_sources
                    .iter()
                    .any(|thread| gecko::worker_source_matches(thread, origin))
            });
            if !observed {
                warnings.push("known worker realm has no matching source identity in the global preset; absent thread and sampling silence cannot be distinguished".into());
            }
        }
    }
    Ok(warnings)
}

pub fn extract(
    body: &Value,
    subject: &str,
    limits: Limits,
    report: Option<Value>,
) -> Result<BrowserExtraction> {
    let kind = body["kind"]
        .as_str()
        .ok_or_else(|| Error("missing browser source kind".into()))?;
    let mut collection = Extraction::new(limits, json!({"interpretation":"browser-collection-v2"}));
    collection.schemas = collection_schemas();
    collection.warnings = coverage_warnings(body, kind)?;
    collection.capture_complete = collection.warnings.is_empty();
    if body["sourceFailures"]
        .as_array()
        .is_some_and(|v| !v.is_empty())
    {
        collection.capture_complete = false;
        collection.warnings.push(
            "one or more declared browser sources failed to arm or export; see collection metadata"
                .into(),
        );
    }
    let metadata: serde_json::Map<String, Value> = body
        .as_object()
        .ok_or_else(|| Error("browser source must be an object".into()))?
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "profiles" | "profile" | "scripts"))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    collection.push(record(
        "collection".into(),
        "browser.collection",
        subject,
        json!({"kind":kind,"metadata":metadata,"coverage":{"complete":collection.capture_complete,"warnings":collection.warnings,"meaning":"declared source export coverage, not exclusive selected-page execution"}}),
    ))?;
    identity_records(&mut collection, body, subject)?;
    if let Some(report) = report {
        if !report["outcome"].is_string() {
            return Err(Error("browser report needs an application outcome".into()));
        }
        collection.push(record(
            "prepared-report".into(),
            "prepared.report",
            subject,
            report,
        ))?;
    }
    for (index, script) in body["scripts"].as_array().into_iter().flatten().enumerate() {
        let mut metadata = script
            .as_object()
            .cloned()
            .ok_or_else(|| Error("invalid executed script descriptor".into()))?;
        let source = match metadata.remove("source") {
            Some(Value::String(source)) => source,
            _ => return Err(Error("executed script bytes missing".into())),
        };
        if source.len() > 512 * 1024 {
            return Err(Error("script content exceeds identity budget".into()));
        }
        let id = script["scriptId"]
            .as_str()
            .ok_or_else(|| Error("script id missing".into()))?;
        let hash = script["sha256"]
            .as_str()
            .ok_or_else(|| Error("script hash missing".into()))?;
        if crate::source_maps::digest(source.as_bytes()) != hash {
            return Err(Error("executed script content/hash conflict".into()));
        }
        collection.push(record(
            format!("script:{index}"),
            "browser.script",
            subject,
            json!({"script_id":id,"sha256":hash,"metadata":metadata}),
        ))?;
        let mut start = 0;
        let mut chunk = 0;
        while start < source.len() {
            let mut end = (start + 32 * 1024).min(source.len());
            while !source.is_char_boundary(end) {
                end -= 1;
            }
            collection.push(record(
                format!("script:{index}:chunk:{chunk}"),
                "browser.script-chunk",
                subject,
                json!({"script_id":id,"index":chunk,"text":&source[start..end]}),
            ))?;
            start = end;
            chunk += 1;
        }
    }
    let mut remaining_limits = limits;
    remaining_limits.max_records = limits
        .max_records
        .checked_sub(collection.records.len())
        .ok_or_else(|| Error("browser collection exceeds record budget".into()))?;
    remaining_limits.max_output_bytes = limits
        .max_output_bytes
        .checked_sub(collection.output_bytes)
        .ok_or_else(|| Error("browser collection exceeds output budget".into()))?;
    let streams = match kind {
        "chromium" => {
            let profiles = body["profiles"]
                .as_array()
                .ok_or_else(|| Error("chromium profiles missing".into()))?;
            if profiles.is_empty() || profiles.len() > 128 {
                return Err(Error("chromium needs 1..=128 target profiles".into()));
            }
            let mut seen = std::collections::BTreeSet::new();
            let mut streams = Vec::new();
            for profile in profiles {
                let target = profile["targetId"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 128)
                    .ok_or_else(|| Error("invalid profiler target id".into()))?;
                if !seen.insert(target) {
                    return Err(Error("duplicate browser target profile".into()));
                }
                let mut extracted = v8::extract(
                    &serde_json::to_vec(&profile["profile"]).map_err(|e| Error(e.to_string()))?,
                    &format!("profile-{target}"),
                    remaining_limits,
                )?;
                cdp::project_contexts(&mut extracted, body, profile)?;
                let mut binding = profile
                    .as_object()
                    .ok_or_else(|| Error("invalid target profile binding".into()))?
                    .clone();
                binding.remove("profile");
                let metadata = extracted
                    .metadata
                    .as_object_mut()
                    .ok_or_else(|| Error("invalid v8 source metadata".into()))?;
                metadata.insert("browser".into(), json!("chromium"));
                metadata.insert("target_id".into(), json!(target));
                metadata.insert("session_id".into(), profile["sessionId"].clone());
                metadata.insert("target_binding".into(), Value::Object(binding));
                metadata.insert(
                    "clock_scope".into(),
                    json!("this exported target profile; no implicit cross-target alignment"),
                );
                metadata.insert("version".into(), body["version"].clone());
                remaining_limits.max_records = remaining_limits
                    .max_records
                    .checked_sub(extracted.records.len())
                    .ok_or_else(|| Error("browser streams exceed record budget".into()))?;
                remaining_limits.max_output_bytes = remaining_limits
                    .max_output_bytes
                    .checked_sub(extracted.output_bytes)
                    .ok_or_else(|| Error("browser streams exceed output budget".into()))?;
                streams.push(BrowserStream {
                    key: format!("target-{target}"),
                    process_id: None,
                    extraction: extracted,
                    interpretation: "browser-v8-v2",
                });
            }
            streams
        }
        "firefox" => gecko::extract(&body["profile"], subject, remaining_limits)?,
        "safari" => vec![BrowserStream {
            key: "instrumented-calls".into(),
            process_id: None,
            interpretation: "browser-safari-instrument-v1",
            extraction: safari::extract(
                &serde_json::to_vec(&body["profile"]).map_err(|e| Error(e.to_string()))?,
                subject,
                remaining_limits,
            )?,
        }],
        _ => return Err(Error("unsupported browser source kind".into())),
    };
    Ok(BrowserExtraction {
        collection,
        streams,
    })
}

fn read_owned(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|e| Error(e.to_string()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Error("browser input must be an owned regular file".into()));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|e| Error(e.to_string()))?;
    if !file.metadata().map_err(|e| Error(e.to_string()))?.is_file() {
        return Err(Error("browser input changed type".into()));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error(e.to_string()))?;
    if bytes.len() > limit {
        return Err(Error("browser input exceeds byte budget".into()));
    }
    Ok(bytes)
}

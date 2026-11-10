use crate::{
    Error, Extraction, Limits, Result, clr, jfr, php, pprof, python, record, ruby, schema, v8,
};
use hunin_core::{
    execution::PreparedLaunch,
    project::TargetRef,
    records::{Clock, FieldType, RecordSchema, SourceDefinition},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeKind {
    Node,
    Deno,
    Bun,
    CPython,
    PyPy,
    CRuby,
    HotSpot,
    Go,
    PHP,
    CoreCLR,
}

#[derive(Debug)]
pub struct PreparedRuntime {
    pub launch: PreparedLaunch,
    pub profile_path: PathBuf,
    pub report_path: PathBuf,
    pub kind: RuntimeKind,
}

pub fn prepare(
    kind: RuntimeKind,
    executable: &Path,
    workload: &Path,
    area: &Path,
) -> Result<PreparedRuntime> {
    if kind == RuntimeKind::CoreCLR {
        return clr::prepare(executable, workload, area);
    }
    if ![executable, workload, area].iter().all(|p| p.is_absolute()) {
        return Err(Error("prepared runtime paths must be absolute".into()));
    }
    if !fs::symlink_metadata(area)
        .map_err(|e| Error(e.to_string()))?
        .file_type()
        .is_dir()
    {
        return Err(Error(
            "prepared area must be an owned directory, not a symlink".into(),
        ));
    }
    let area = fs::canonicalize(area).map_err(|e| Error(e.to_string()))?;
    let executable = fs::canonicalize(executable).map_err(|e| Error(e.to_string()))?;
    let workload = fs::canonicalize(workload).map_err(|e| Error(e.to_string()))?;
    if [area.as_path(), workload.as_path()]
        .iter()
        .any(|path| path.to_str().is_none())
    {
        return Err(Error("prepared helper paths must be UTF-8".into()));
    }
    let (name, helper) = match kind {
        RuntimeKind::Node | RuntimeKind::Deno | RuntimeKind::Bun => {
            ("inspector.cjs", crate::helpers::INSPECTOR)
        }
        RuntimeKind::CPython | RuntimeKind::PyPy => ("cprofile.py", crate::helpers::PYTHON),
        RuntimeKind::CRuby => ("tracepoint.rb", crate::helpers::RUBY),
        RuntimeKind::HotSpot => ("HuninJfr.java", crate::helpers::JFR),
        RuntimeKind::Go => ("hunin.go", crate::helpers::GO),
        RuntimeKind::PHP => {
            return Err(Error(
                "PHP preparation requires an explicit Xdebug extension path; use php::prepare"
                    .into(),
            ));
        }
        RuntimeKind::CoreCLR => {
            return Err(Error(
                "CoreCLR preparation must use its diagnostic bootstrap".into(),
            ));
        }
    };
    let helper_path = area.join(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&helper_path)
        .map_err(|e| Error(e.to_string()))?;
    file.write_all(helper.as_bytes())
        .map_err(|e| Error(e.to_string()))?;
    let profile_path = area.join(if kind == RuntimeKind::Go {
        "runtime-profile.pprof"
    } else {
        "runtime-profile.json"
    });
    let report_path = area.join("runtime-report.json");
    if profile_path.exists() || report_path.exists() {
        return Err(Error("prepared output paths already exist".into()));
    }
    let mut args = vec![];
    if kind == RuntimeKind::Deno {
        if [area.as_path(), workload.parent().unwrap()]
            .iter()
            .any(|path| path.to_string_lossy().contains(','))
        {
            return Err(Error(
                "Deno permission paths containing commas need a separately encoded grant".into(),
            ));
        }
        args.extend([
            "run".into(),
            "--allow-sys=inspector".into(),
            "--allow-env=HUNIN_FINALIZE_GATE,HUNIN_SCRIPT_SOURCES".into(),
            format!(
                "--allow-read={},{}",
                area.display(),
                workload.parent().unwrap().display()
            ),
            format!("--allow-write={}", area.display()),
        ]);
    }
    let program = if kind == RuntimeKind::Go {
        let source_name = workload.to_string_lossy();
        if source_name.contains(['\n', '\r']) {
            return Err(Error("Go source path contains a newline".into()));
        }
        let source = area.join("workload.go");
        let bytes = bounded_file(&workload, 4 * 1024 * 1024)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&source)
            .map_err(|e| Error(e.to_string()))?;
        writeln!(file, "//line {source_name}:1").map_err(|e| Error(e.to_string()))?;
        file.write_all(&bytes).map_err(|e| Error(e.to_string()))?;
        let output = area.join(if cfg!(windows) {
            "hunin-go.exe"
        } else {
            "hunin-go"
        });
        compile_go(&executable, &helper_path, &source, &output, &area)?;
        args.extend([
            profile_path.to_string_lossy().into_owned(),
            report_path.to_string_lossy().into_owned(),
        ]);
        output
    } else if kind == RuntimeKind::HotSpot {
        if area.to_string_lossy().contains(',') {
            return Err(Error("JFR repository path cannot contain a comma".into()));
        }
        let compiler = executable.with_file_name(if cfg!(windows) { "javac.exe" } else { "javac" });
        compile_java(&compiler, &helper_path, &workload, &area)?;
        let manifest = area.join("class-bindings.txt");
        let mut bindings = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&manifest)
            .map_err(|e| Error(e.to_string()))?;
        writeln!(
            bindings,
            "source {:x}",
            Sha256::digest(bounded_file(&workload, 4 * 1024 * 1024)?)
        )
        .map_err(|e| Error(e.to_string()))?;
        for entry in fs::read_dir(&area).map_err(|e| Error(e.to_string()))? {
            let path = entry.map_err(|e| Error(e.to_string()))?.path();
            if path.extension().is_some_and(|ext| ext == "class") {
                let name = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.chars().any(char::is_whitespace))
                    .ok_or_else(|| Error("invalid Java class manifest name".into()))?;
                writeln!(
                    bindings,
                    "class {name} {:x}",
                    Sha256::digest(bounded_file(&path, 1024 * 1024)?)
                )
                .map_err(|e| Error(e.to_string()))?;
            }
        }
        bindings.sync_all().map_err(|e| Error(e.to_string()))?;
        let class = workload
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Error("invalid prepared Java class name".into()))?;
        if class.is_empty()
            || !class
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        {
            return Err(Error("invalid prepared Java class name".into()));
        }
        let repository = area.join("jfr-repository");
        fs::create_dir(&repository).map_err(|e| Error(e.to_string()))?;
        args.extend([
            "-XX:-UsePerfData".into(),
            format!("-Djava.io.tmpdir={}", area.display()),
            format!(
                "-XX:FlightRecorderOptions=repository={}",
                repository.display()
            ),
            "-cp".into(),
            area.to_string_lossy().into_owned(),
            "HuninJfr".into(),
            class.into(),
            workload.to_string_lossy().into_owned(),
            area.join("recording.jfr").to_string_lossy().into_owned(),
            profile_path.to_string_lossy().into_owned(),
            report_path.to_string_lossy().into_owned(),
            manifest.to_string_lossy().into_owned(),
        ]);
        executable.clone()
    } else {
        args.extend(
            [&helper_path, &workload, &profile_path, &report_path]
                .map(|p| p.to_string_lossy().into_owned()),
        );
        executable.clone()
    };
    let mut environment = BTreeMap::from([
        ("TMPDIR".into(), area.to_string_lossy().into_owned()),
        ("TEMP".into(), area.to_string_lossy().into_owned()),
        ("TMP".into(), area.to_string_lossy().into_owned()),
    ]);
    match kind {
        RuntimeKind::CPython | RuntimeKind::PyPy => {
            environment.insert("PYTHONDONTWRITEBYTECODE".into(), "1".into());
        }
        RuntimeKind::Deno => {
            let cache = area.join("deno-cache");
            fs::create_dir(&cache).map_err(|e| Error(e.to_string()))?;
            environment.insert("DENO_DIR".into(), cache.to_string_lossy().into_owned());
        }
        RuntimeKind::Bun => {
            let cache = area.join("bun-cache");
            fs::create_dir(&cache).map_err(|e| Error(e.to_string()))?;
            environment.insert(
                "BUN_RUNTIME_TRANSPILER_CACHE_PATH".into(),
                cache.to_string_lossy().into_owned(),
            );
        }
        _ => {}
    }
    Ok(PreparedRuntime {
        launch: PreparedLaunch {
            program,
            args,
            directory: area.to_path_buf(),
            environment,
            stdout_limit: 64 * 1024,
        },
        profile_path,
        report_path,
        kind,
    })
}

impl PreparedRuntime {
    /// grants only the caller's explicit target environment and filesystem paths.
    pub fn configure(
        &mut self,
        environment: &BTreeMap<String, String>,
        read_paths: &[PathBuf],
        write_paths: &[PathBuf],
    ) -> Result<()> {
        let same_key = |left: &str, right: &str| {
            if cfg!(windows) {
                left.eq_ignore_ascii_case(right)
            } else {
                left == right
            }
        };
        for (key, value) in environment {
            if same_key(key, "HUNIN_FINALIZE_GATE") {
                return Err(Error(
                    "HUNIN_FINALIZE_GATE is owned by the prepared process lifecycle".into(),
                ));
            }
            if self
                .launch
                .environment
                .iter()
                .any(|(owned, current)| same_key(owned, key) && current != value)
            {
                return Err(Error(format!(
                    "prepared environment {key} is owned by its collector"
                )));
            }
        }
        if environment.iter().any(|(key, value)| {
            key.is_empty()
                || key.contains(['=', '\0'])
                || (self.kind == RuntimeKind::Deno && key.contains(','))
                || value.contains('\0')
        }) {
            return Err(Error("invalid prepared environment key/value".into()));
        }
        let mut reads = vec![];
        let mut writes = vec![];
        for (paths, grants) in [(read_paths, &mut reads), (write_paths, &mut writes)] {
            for path in paths {
                if !path.is_absolute() {
                    return Err(Error("prepared filesystem grants must be absolute".into()));
                }
                if self.kind == RuntimeKind::Deno {
                    let lexical = path
                        .to_str()
                        .ok_or_else(|| Error("prepared permission path is not UTF-8".into()))?;
                    if lexical.contains(',') {
                        return Err(Error("Deno permission paths containing commas need a separately encoded grant".into()));
                    }
                    grants.push(lexical.to_string());
                }
                let path = fs::canonicalize(path).map_err(|e| Error(e.to_string()))?;
                let text = path
                    .to_str()
                    .ok_or_else(|| Error("prepared permission path is not UTF-8".into()))?;
                if self.kind == RuntimeKind::Deno && text.contains(',') {
                    return Err(Error(
                        "Deno permission paths containing commas need a separately encoded grant"
                            .into(),
                    ));
                }
                grants.push(text.to_string());
            }
        }
        if self.kind == RuntimeKind::Deno {
            let mut args = self.launch.args.clone();
            for (prefix, additional) in [
                (
                    "--allow-env=",
                    environment.keys().cloned().collect::<Vec<_>>(),
                ),
                ("--allow-read=", reads),
                ("--allow-write=", writes),
            ] {
                let arg = args
                    .iter_mut()
                    .find(|arg| arg.starts_with(prefix))
                    .ok_or_else(|| Error("missing prepared Deno permission declaration".into()))?;
                let mut all = arg[prefix.len()..]
                    .split(',')
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                all.extend(additional);
                all.sort();
                all.dedup();
                *arg = format!("{prefix}{}", all.join(","));
            }
            self.launch.args = args;
        }
        self.launch.environment.extend(environment.clone());
        Ok(())
    }
    pub fn schemas(&self) -> Vec<RecordSchema> {
        let mut schemas = match self.kind {
            RuntimeKind::Node | RuntimeKind::Deno | RuntimeKind::Bun => v8::schemas(),
            RuntimeKind::CPython | RuntimeKind::PyPy => python::schemas(),
            RuntimeKind::CRuby => ruby::schemas(),
            RuntimeKind::HotSpot => jfr::schemas(),
            RuntimeKind::Go => pprof::schemas(),
            RuntimeKind::PHP => php::schemas(),
            RuntimeKind::CoreCLR => clr::schemas(),
        };
        schemas.push(schema(
            "prepared.report",
            "prepared application outcome and actual runtime identity",
            &[(
                "outcome",
                FieldType::String,
                None,
                "application completion distinct from collection success",
            )],
        ));
        schemas
    }

    pub fn clocks(&self) -> Vec<Clock> {
        match self.kind {
            RuntimeKind::Node | RuntimeKind::Deno | RuntimeKind::Bun => v8::clocks(),
            RuntimeKind::CPython | RuntimeKind::PyPy => vec![],
            RuntimeKind::CRuby => ruby::clocks(),
            RuntimeKind::HotSpot => jfr::clocks(),
            RuntimeKind::Go => vec![],
            RuntimeKind::PHP => vec![],
            RuntimeKind::CoreCLR => clr::clocks(),
        }
    }

    pub fn definition(&self, id: &str, target: &TargetRef) -> SourceDefinition {
        SourceDefinition {
            id: id.into(),
            target: Some(target.clone()),
            schemas: self.schemas(),
            clocks: self.clocks(),
            interpretation: "runtime-extractor-v1".into(),
            metadata: json!({"runtime_kind":format!("{:?}", self.kind),
                "selected_executable": self.launch.program, "scope":"prepared process bootstrap; additional threads/isolates require explicit coverage"}),
        }
    }

    pub fn extract(&self, subject: &str, limits: Limits) -> Result<Extraction> {
        let bytes = bounded_file(&self.profile_path, limits.max_input_bytes)?;
        let report: Value =
            serde_json::from_slice(&bounded_file(&self.report_path, limits.max_input_bytes)?)
                .map_err(|e| Error(e.to_string()))?;
        if !report["outcome"].is_string() {
            return Err(Error("prepared report has no application outcome".into()));
        }
        let mut extraction = match self.kind {
            RuntimeKind::Node | RuntimeKind::Deno | RuntimeKind::Bun => {
                let runtime = &report["runtime"];
                let matches = match self.kind {
                    RuntimeKind::Node => {
                        runtime["node"].is_string()
                            && runtime["deno"].is_null()
                            && runtime["bun"].is_null()
                    }
                    RuntimeKind::Deno => runtime["deno"].is_string(),
                    RuntimeKind::Bun => runtime["bun"].is_string(),
                    _ => unreachable!(),
                };
                if !matches {
                    return Err(Error(
                        "actual runtime does not match requested support".into(),
                    ));
                }
                v8::extract(&bytes, subject, limits)?
            }
            RuntimeKind::CPython | RuntimeKind::PyPy => {
                let extracted = python::extract(&bytes, subject, limits)?;
                let profile = extracted
                    .records
                    .iter()
                    .find(|r| r.id == "profile")
                    .unwrap();
                let expected = if self.kind == RuntimeKind::CPython {
                    "cpython"
                } else {
                    "pypy"
                };
                if profile.payload["runtime"] != expected {
                    return Err(Error(
                        "actual python implementation differs from request".into(),
                    ));
                }
                extracted
            }
            RuntimeKind::CRuby => ruby::extract(&bytes, subject, limits)?,
            RuntimeKind::HotSpot => jfr::extract(&bytes, subject, limits)?,
            RuntimeKind::Go => pprof::extract(&bytes, subject, limits)?,
            RuntimeKind::PHP => php::extract(&bytes, subject, limits)?,
            RuntimeKind::CoreCLR => clr::extract(&bytes, subject, limits)?,
        };
        extraction.schemas = self.schemas();
        extraction.push(record(
            "prepared-report".into(),
            "prepared.report",
            subject,
            report,
        ))?;
        Ok(extraction)
    }
}

fn compile_java(compiler: &Path, helper: &Path, workload: &Path, area: &Path) -> Result<()> {
    let mut command = std::process::Command::new(compiler);
    command
        .args(["-g", "-proc:none", "-d"])
        .arg(area)
        .arg(helper)
        .arg(workload)
        .current_dir(area);
    let output = hunin_platforms::tools::run(
        &mut command,
        area,
        std::time::Duration::from_secs(30),
        64 * 1024,
    )
    .map_err(|e| Error(e.to_string()))?;
    hunin_platforms::tools::require_success(output)
        .map_err(|e| Error(format!("javac preparation failed: {e}")))?;
    Ok(())
}

pub(crate) fn bounded_file(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![];
    if !fs::symlink_metadata(path)
        .map_err(|e| Error(e.to_string()))?
        .file_type()
        .is_file()
    {
        return Err(Error(
            "runtime output must be a regular file, not a symlink".into(),
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path).map_err(|e| Error(e.to_string()))?;
    if !file.metadata().map_err(|e| Error(e.to_string()))?.is_file() {
        return Err(Error("runtime output is not a regular file".into()));
    }
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error(e.to_string()))?;
    if bytes.len() > limit {
        return Err(Error("runtime output byte budget exceeded".into()));
    }
    Ok(bytes)
}

fn compile_go(
    compiler: &Path,
    helper: &Path,
    workload: &Path,
    output: &Path,
    area: &Path,
) -> Result<()> {
    for name in ["go-cache", "go-mod-cache", "go-tmp"] {
        fs::create_dir(area.join(name)).map_err(|e| Error(e.to_string()))?;
    }
    let mut command = std::process::Command::new(compiler);
    command
        .args(["build", "-o"])
        .arg(output)
        .arg(helper)
        .arg(workload)
        .env("GOCACHE", area.join("go-cache"))
        .env("GOMODCACHE", area.join("go-mod-cache"))
        .env("GOTMPDIR", area.join("go-tmp"))
        .env("GOENV", "off")
        .env("GOTOOLCHAIN", "local")
        .env("GO111MODULE", "off")
        .current_dir(area);
    let output = hunin_platforms::tools::run(
        &mut command,
        area,
        std::time::Duration::from_secs(60),
        64 * 1024,
    )
    .map_err(|e| Error(e.to_string()))?;
    hunin_platforms::tools::require_success(output)
        .map_err(|e| Error(format!("Go preparation failed: {e}")))?;
    Ok(())
}

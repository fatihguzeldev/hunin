use super::Language;
use crate::{
    process::{Error, ErrorKind, Result},
    tools,
};
use hunin_core::{
    project::TargetRef,
    records::{FieldSchema, FieldType, Record, RecordSchema, SourceDefinition},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

#[derive(Debug, Clone)]
pub struct CoverageBuild {
    pub binary: PathBuf,
    pub sha256: String,
    pub language: Language,
    pub profdata: PathBuf,
    pub cov: PathBuf,
    pub profdata_version: String,
    pub cov_version: String,
}
#[derive(Debug, Clone)]
pub struct CoverageSnapshot {
    pub format_version: String,
    pub binary_sha256: String,
    pub language: Language,
    pub phase: String,
    pub producer: Value,
    records: Vec<Record>,
}
pub fn sha256(path: &Path) -> Result<String> {
    let mut input = File::open(path)?;
    let length = input.metadata()?.len();
    if !input.metadata()?.is_file() {
        return Err(Error::invalid("hash input must be a regular file"));
    }
    if length > 1024 * 1024 * 1024 {
        return Err(Error::new(
            ErrorKind::LimitExceeded,
            "binary exceeds 1 gib hashing budget",
        ));
    }
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    let mut total = 0u64;
    loop {
        let count = input.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > 1024 * 1024 * 1024 {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "hash input grew beyond 1 gib",
            ));
        }
        hash.update(&bytes[..count]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
impl CoverageBuild {
    pub fn bind(
        binary: &Path,
        language: Language,
        profdata: &Path,
        cov: &Path,
        temporary: &Path,
    ) -> Result<Self> {
        if !binary.is_absolute() || !profdata.is_absolute() || !cov.is_absolute() {
            return Err(Error::invalid(
                "coverage tools and binary must have absolute paths",
            ));
        }
        let profdata_version = tools::require_success(tools::run(
            Command::new(profdata).arg("--version"),
            temporary,
            Duration::from_secs(10),
            65536,
        )?)?
        .stdout;
        let cov_version = tools::require_success(tools::run(
            Command::new(cov).arg("--version"),
            temporary,
            Duration::from_secs(10),
            65536,
        )?)?
        .stdout;
        Ok(Self {
            binary: binary.into(),
            sha256: sha256(binary)?,
            language,
            profdata: profdata.into(),
            cov: cov.into(),
            profdata_version,
            cov_version,
        })
    }
    pub fn export_owned_capture(
        &self,
        raw: &Path,
        phase: &str,
        temporary: &Path,
    ) -> Result<CoverageSnapshot> {
        if self.sha256 != sha256(&self.binary)? {
            return Err(Error::new(
                ErrorKind::SymbolMismatch,
                "coverage target binary changed after capture binding",
            ));
        }
        if !raw.is_absolute() || std::fs::metadata(raw)?.len() > 256 * 1024 * 1024 {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "coverage raw input must be absolute and at most 256 mib",
            ));
        }
        let raw_sha256 = sha256(raw)?;
        let id =
            hunin_core::project::Project::new_id().map_err(|e| Error::invalid(e.to_string()))?;
        let merged = temporary.join(format!("coverage-{id}.profdata"));
        let result = (|| {
            tools::require_success(tools::run(
                Command::new(&self.profdata)
                    .args(["merge", "-sparse"])
                    .arg(raw)
                    .arg("-o")
                    .arg(&merged),
                temporary,
                Duration::from_secs(30),
                1024 * 1024,
            )?)?;
            let export = tools::require_success(tools::run(
                Command::new(&self.cov)
                    .arg("export")
                    .arg(&self.binary)
                    .arg(format!("-instr-profile={}", merged.display())),
                temporary,
                Duration::from_secs(30),
                64 * 1024 * 1024,
            )?)?;
            if !export.stderr.trim().is_empty() {
                return Err(Error::new(
                    ErrorKind::CollectorFailure,
                    format!(
                        "llvm coverage export diagnostics require explicit interpretation: {}",
                        export.stderr
                    ),
                ));
            }
            if raw_sha256 != sha256(raw)? {
                return Err(Error::new(
                    ErrorKind::CollectorFailure,
                    "coverage raw capture changed during extraction",
                ));
            }
            if self.sha256 != sha256(&self.binary)? {
                return Err(Error::new(
                    ErrorKind::SymbolMismatch,
                    "coverage binary changed during extraction",
                ));
            }
            parse_export(
                &export.stdout,
                &self.sha256,
                self.language,
                phase,
                json!({"profdata":self.profdata,"profdata_version":self.profdata_version,"llvm_cov":self.cov,"llvm_cov_version":self.cov_version,"raw_sha256":raw_sha256,"raw_origin":"caller-owned capture from the hash-bound executable; arbitrary external raw files do not establish build provenance"}),
            )
        })();
        let _ = std::fs::remove_file(merged);
        result
    }
}
fn object(value: &Value) -> Result<&serde_json::Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| Error::invalid("coverage object expected"))
}
fn array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value]> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| {
            Error::new(
                ErrorKind::UnsupportedFormat,
                format!("coverage missing array:{key}"),
            )
        })
}
fn uint(value: &Value, key: &str) -> Result<u64> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::invalid(format!("coverage invalid unsigned field:{key}")))
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid(format!("coverage missing string:{key}")))
}
fn fields_without(value: &Value, excluded: &[&str]) -> Result<Value> {
    Ok(Value::Object(
        object(value)?
            .iter()
            .filter(|(key, _)| !excluded.contains(&key.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    ))
}
fn source_position(row: &[Value], file: &str) -> Result<Option<String>> {
    let line = row
        .first()
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::invalid("coverage line must be unsigned"))?;
    if line == 0 {
        Ok(None)
    } else {
        Ok(Some(format!("{file}:{line}")))
    }
}
fn numeric_row(value: &Value, length: usize) -> Result<&[Value]> {
    let row = value
        .as_array()
        .ok_or_else(|| Error::invalid("coverage tuple expected"))?;
    if row.len() != length || row.iter().any(|v| v.as_u64().is_none()) {
        return Err(Error::new(
            ErrorKind::UnsupportedFormat,
            "unsupported coverage tuple layout",
        ));
    }
    Ok(row)
}

pub fn parse_export(
    text: &str,
    binary_sha256: &str,
    language: Language,
    phase: &str,
    producer: Value,
) -> Result<CoverageSnapshot> {
    if text.len() > 64 * 1024 * 1024
        || phase.is_empty()
        || phase.len() > 64
        || !phase
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(Error::new(
            ErrorKind::LimitExceeded,
            "invalid coverage phase or input budget",
        ));
    }
    if binary_sha256.len() != 64 || !binary_sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::invalid(
            "coverage requires capture-bound binary sha256",
        ));
    }
    let root: Value = serde_json::from_str(text)
        .map_err(|e| Error::new(ErrorKind::UnsupportedFormat, e.to_string()))?;
    let format_version = root
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if root.get("type").and_then(Value::as_str) != Some("llvm.coverage.json.export")
        || !["3.0.1", "3.1.0"].contains(&format_version.as_str())
    {
        return Err(Error::new(
            ErrorKind::UnsupportedFormat,
            format!(
                "only llvm coverage json export 3.0.1 and 3.1.0 are interpreted; got type={} version={}",
                root["type"], root["version"]
            ),
        ));
    }
    let mut records = Vec::new();
    let mut size = 0usize;
    let mut push = |id: String,
                    schema: &str,
                    function: Option<&str>,
                    location: Option<String>,
                    mut payload: Value|
     -> Result<()> {
        payload["format_version"] = format_version.clone().into();
        payload["phase"] = phase.into();
        payload["binary_sha256"] = binary_sha256.into();
        let record = Record {
            id: format!("{phase}-{id}"),
            schema: format!("native.llvm_coverage.{schema}"),
            schema_version: 1,
            subject: "target".into(),
            time: None,
            function: function.map(str::to_string),
            location,
            operation: None,
            payload,
        };
        let bytes = serde_json::to_vec(&record).map_err(|e| Error::invalid(e.to_string()))?;
        if bytes.len() > hunin_core::project::MAX_VALUE_BYTES {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "coverage record exceeds durable value limit",
            ));
        }
        size += bytes.len();
        if size > 64 * 1024 * 1024 || records.len() > 1_000_000 {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "coverage extraction exceeds record budget",
            ));
        }
        records.push(record);
        Ok(())
    };
    push(
        "capture".into(),
        "capture",
        None,
        None,
        json!({"format":"llvm.coverage.json.export","version":format_version,"producer":producer,"extra_top_level":fields_without(&root,&["type","version","data"])?}),
    )?;
    for (group, data) in array(&root, "data")?.iter().enumerate() {
        for (fi, function) in array(data, "functions")?.iter().enumerate() {
            let name = string(function, "name")?;
            uint(function, "count")?;
            let filenames = array(function, "filenames")?;
            if filenames.iter().any(|f| f.as_str().is_none()) {
                return Err(Error::invalid("coverage filename must be string"));
            }
            let parent = format!("g{group}-function-{fi}");
            let mut header = fields_without(function, &["regions", "branches", "mcdc_records"])?;
            header["function_id"] = format!("{phase}-{parent}").into();
            push(parent.clone(), "function", Some(name), None, header)?;
            for (i, value) in array(function, "regions")?.iter().enumerate() {
                let row = numeric_row(value, 8)?;
                let file_id = usize::try_from(row[5].as_u64().unwrap())
                    .map_err(|_| Error::invalid("coverage file id overflow"))?;
                let file = filenames
                    .get(file_id)
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::invalid("coverage region filename id missing"))?;
                push(
                    format!("{parent}-region-{i}"),
                    "region",
                    Some(name),
                    source_position(row, file)?,
                    json!({"function_id":format!("{phase}-{parent}"),"start_line":row[0],"start_column":row[1],"end_line":row[2],"end_column":row[3],"count":row[4],"file_id":row[5],"expanded_file_id":row[6],"kind":row[7]}),
                )?;
            }
            for (kind, length) in [("branches", 9), ("mcdc_records", 0)] {
                if let Some(values) = function.get(kind) {
                    for (i, value) in values
                        .as_array()
                        .ok_or_else(|| Error::invalid("coverage branch/mcdc list expected"))?
                        .iter()
                        .enumerate()
                    {
                        if length > 0 {
                            numeric_row(value, length)?;
                        }
                        push(
                            format!("{parent}-{kind}-{i}"),
                            if kind == "branches" { "branch" } else { "mcdc" },
                            Some(name),
                            None,
                            json!({"function_id":format!("{phase}-{parent}"),"value":if kind=="mcdc_records"{json!({"source_value":value})}else{value.clone()}}),
                        )?;
                    }
                }
            }
        }
        for (fi, file) in array(data, "files")?.iter().enumerate() {
            let filename = string(file, "filename")?;
            let parent = format!("g{group}-file-{fi}");
            let mut header = fields_without(
                file,
                &["segments", "branches", "mcdc_records", "expansions"],
            )?;
            header["file_id"] = format!("{phase}-{parent}").into();
            push(parent.clone(), "file", None, Some(filename.into()), header)?;
            for (kind, schema) in [
                ("segments", "segment"),
                ("branches", "file_branch"),
                ("mcdc_records", "file_mcdc"),
                ("expansions", "expansion"),
            ] {
                if let Some(values) = file.get(kind) {
                    for (i, value) in values
                        .as_array()
                        .ok_or_else(|| Error::invalid("coverage file-detail list expected"))?
                        .iter()
                        .enumerate()
                    {
                        let location = if kind == "segments" {
                            let row = value
                                .as_array()
                                .ok_or_else(|| Error::invalid("coverage segment tuple expected"))?;
                            if row.len() != 6
                                || row[..3].iter().any(|v| v.as_u64().is_none())
                                || row[3..].iter().any(|v| v.as_bool().is_none())
                            {
                                return Err(Error::new(
                                    ErrorKind::UnsupportedFormat,
                                    "unsupported coverage segment layout",
                                ));
                            }
                            source_position(row, filename)?
                        } else {
                            None
                        };
                        push(
                            format!("{parent}-{kind}-{i}"),
                            schema,
                            None,
                            location,
                            json!({"file_id":format!("{phase}-{parent}"),"value":if kind=="mcdc_records"{json!({"source_value":value})}else{value.clone()}}),
                        )?;
                    }
                }
            }
        }
        push(
            format!("g{group}-totals"),
            "totals",
            None,
            None,
            fields_without(data, &["functions", "files"])?,
        )?;
    }
    if !records
        .iter()
        .any(|r| r.schema == "native.llvm_coverage.function")
    {
        return Err(Error::new(
            ErrorKind::CollectorFailure,
            "coverage export contains no instrumented functions",
        ));
    }
    Ok(CoverageSnapshot {
        format_version: format_version.clone(),
        binary_sha256: binary_sha256.into(),
        language,
        phase: phase.into(),
        producer: json!({"format":"llvm.coverage.json.export","version":format_version,"toolchain":producer,"extra_top_level":fields_without(&root,&["type","version","data"])?}),
        records,
    })
}
impl CoverageSnapshot {
    pub fn records(&self, subject: &str) -> Vec<Record> {
        self.records
            .iter()
            .map(|record| {
                let mut record = record.clone();
                record.subject = subject.into();
                record
            })
            .collect()
    }
    pub fn definition(&self, id: &str, target: TargetRef) -> SourceDefinition {
        let mut schemas = Vec::new();
        for kind in [
            "capture",
            "function",
            "region",
            "branch",
            "mcdc",
            "file",
            "segment",
            "file_branch",
            "file_mcdc",
            "expansion",
            "totals",
        ] {
            let mut fields=BTreeMap::from([("format_version".into(),FieldSchema{kind:FieldType::String,required:true,unit:None,meaning:"actual llvm coverage export version; MC/DC details remain represented in the original versioned source value".into()}),("phase".into(),FieldSchema{kind:FieldType::String,required:true,unit:None,meaning:"owned capture snapshot phase; counters are cumulative, not interval durations".into()}),("binary_sha256".into(),FieldSchema{kind:FieldType::String,required:true,unit:None,meaning:"exact executable hash bound by the collector before launch and checked at export".into()})]);
            if kind == "function" || kind == "region" {
                fields.insert(
                    "count".into(),
                    FieldSchema {
                        kind: FieldType::Unsigned,
                        required: true,
                        unit: Some("executions".into()),
                        meaning: if kind == "function" {
                            "compiler entry counter"
                        } else {
                            "compiler coverage-region execution counter"
                        }
                        .into(),
                    },
                );
            }
            if kind == "function" {
                fields.insert(
                    "name".into(),
                    FieldSchema {
                        kind: FieldType::String,
                        required: true,
                        unit: None,
                        meaning: "compiler-recorded function name".into(),
                    },
                );
            }
            if kind == "region" {
                for (name, unit) in [
                    ("start_line", "line"),
                    ("start_column", "column"),
                    ("end_line", "line"),
                    ("end_column", "column"),
                    ("file_id", "index"),
                    ("expanded_file_id", "index"),
                    ("kind", "llvm_region_kind"),
                ] {
                    fields.insert(
                        name.into(),
                        FieldSchema {
                            kind: FieldType::Unsigned,
                            required: true,
                            unit: Some(unit.into()),
                            meaning: "llvm coverage source-region coordinate or discriminator"
                                .into(),
                        },
                    );
                }
            }
            if [
                "branch",
                "mcdc",
                "segment",
                "file_branch",
                "file_mcdc",
                "expansion",
            ]
            .contains(&kind)
            {
                fields.insert("value".into(),FieldSchema{kind:if ["expansion","mcdc","file_mcdc"].contains(&kind){FieldType::Object}else{FieldType::Array},required:true,unit:None,meaning:"versioned llvm source-detail value, retained exactly with its containing file/function reference".into()});
            }
            schemas.push(RecordSchema{name:format!("native.llvm_coverage.{kind}"),version:1,fields,description:"llvm coverage evidence with the recorded source format version; no invocation timestamps, sampling weights, or per-function cpu attribution".into()});
        }
        SourceDefinition {
            id: id.into(),
            target: Some(target),
            schemas,
            clocks: Vec::new(),
            interpretation: format!("hunin-platforms/llvm-coverage-{}/1", self.format_version),
            metadata: json!({"binary_sha256":self.binary_sha256,"declared_build_language":self.language,"producer":self.producer,"counter_scope":"compiler-instrumented functions; snapshots must not be summed","clock":"none: aggregate counters only","external_raw_inputs":"no capture provenance is inferred from filenames or llvm binary ids"}),
        }
    }
}

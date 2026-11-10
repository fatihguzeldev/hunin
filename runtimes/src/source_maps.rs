use crate::{Error, Extraction, Limits, Result, record, schema};
use base64::Engine;
use hunin_core::records::{FieldType, Record};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Binding {
    pub generated_sha256: String,
    pub map_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Source {
    id: u32,
    name: String,
    content: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Segment {
    generated_line: u32,
    generated_column: u32,
    source: Option<u32>,
    original_line: u32,
    original_column: u32,
    name: Option<String>,
}

#[derive(Debug)]
pub struct BoundMap {
    binding: Binding,
    sources: Vec<Source>,
    segments: Vec<Segment>,
    header: Map<String, Value>,
    limits: Limits,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginalPosition {
    pub source: String,
    pub line: u32,
    pub column: u32,
    pub name: Option<String>,
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl BoundMap {
    pub fn binding(&self) -> &Binding {
        &self.binding
    }
    /// bun 1.4.4 remaps cpu-profile definitions to its loaded source before export.
    /// its debugger returns the wrapper bytes and an embedded map containing that source.
    pub fn project_bun(
        &self,
        report: &Value,
        records: &[Record],
        subject: &str,
    ) -> Result<Extraction> {
        if report["runtime"]["bun"].as_str() != Some("1.4.4") {
            return Err(Error(
                "Bun profile coordinate interpretation requires verified 1.4.4".into(),
            ));
        }
        let scripts = report["scripts"]
            .as_array()
            .ok_or_else(|| Error("missing executed Bun scripts".into()))?;
        let mut matching = vec![];
        for script in scripts {
            let text = script["source"]
                .as_str()
                .ok_or_else(|| Error("missing Bun wrapper source".into()))?;
            let hash = script["sha256"]
                .as_str()
                .ok_or_else(|| Error("missing Bun wrapper hash".into()))?;
            let wrapper = Self::embedded(text.as_bytes(), hash, self.limits)?;
            if wrapper.sources.iter().any(|s| {
                s.content.as_ref().is_some_and(|content| {
                    digest(content.as_bytes()) == self.binding.generated_sha256
                })
            }) {
                matching.push((script, wrapper));
            }
        }
        if matching.len() != 1 {
            return Err(Error(
                "Bun wrapper does not bind one matching loaded source".into(),
            ));
        }
        let (script, wrapper) = &matching[0];
        let id = script["scriptId"]
            .as_str()
            .ok_or_else(|| Error("missing Bun runtime script id".into()))?;
        let mut projected =
            self.project_v8(id, &self.binding.generated_sha256, records, subject)?;
        projected.metadata = json!({"coordinate_interpretation":"bun-1.4.4-original-source-definition","runtime_script_sha256":script["sha256"],"runtime_map_sha256":wrapper.binding.map_sha256,"loaded_source_sha256":self.binding.generated_sha256,"map_sha256":self.binding.map_sha256});
        for row in &mut projected.records {
            row.payload["runtime_script_sha256"] = script["sha256"].clone();
            row.payload["runtime_map_sha256"] = json!(wrapper.binding.map_sha256);
            row.payload["coordinate_interpretation"] =
                json!("bun-1.4.4-original-source-definition");
        }
        Ok(projected)
    }
    /// preserves the generated records and returns separate source-mapping evidence.
    /// script id and hash must come from the profiled runtime's executed script.
    pub fn project_v8(
        &self,
        script_id: &str,
        executed_sha256: &str,
        records: &[Record],
        subject: &str,
    ) -> Result<Extraction> {
        if executed_sha256 != self.binding.generated_sha256 {
            return Err(Error(
                "executed script does not match source-map build".into(),
            ));
        }
        let mut result = Extraction::new(
            self.limits,
            json!({"interpretation":"source-map-v3","generated_sha256":executed_sha256,"map_sha256":self.binding.map_sha256}),
        );
        result.schemas = vec![schema(
            "sourcemap.location",
            "original location derived from a build-bound generated runtime frame",
            &[
                (
                    "generated_record",
                    FieldType::String,
                    None,
                    "source-local original v8.node record id",
                ),
                (
                    "script_id",
                    FieldType::String,
                    None,
                    "runtime script id scoped to its code source",
                ),
                (
                    "generated_sha256",
                    FieldType::String,
                    None,
                    "hash of source bytes whose coordinates are mapped; runtime wrapper identity is retained separately",
                ),
                (
                    "map_sha256",
                    FieldType::String,
                    None,
                    "selected map hash from build binding",
                ),
                (
                    "source",
                    FieldType::String,
                    None,
                    "original source identity",
                ),
                (
                    "line",
                    FieldType::Unsigned,
                    None,
                    "zero-based original line",
                ),
                (
                    "column",
                    FieldType::Unsigned,
                    None,
                    "zero-based original utf-16 column",
                ),
            ],
        )];
        for node in records.iter().filter(|r| r.schema == "v8.node") {
            let frame = &node.payload["callFrame"];
            if frame["scriptId"].as_str() != Some(script_id) {
                continue;
            }
            let Some(line) = frame["lineNumber"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
            else {
                continue;
            };
            let Some(column) = frame["columnNumber"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
            else {
                continue;
            };
            let position = self.lookup(line, column);
            let Some(original) = position else {
                continue;
            };
            let mut row = record(
                format!("original:{}", node.id),
                "sourcemap.location",
                subject,
                json!({
                    "generated_record":node.id,"script_id":script_id,"generated_sha256":executed_sha256,"map_sha256":self.binding.map_sha256,
                    "generated_line":line,"generated_column":column,"generated_url":frame["url"],
                    "source":original.source,"line":original.line,"column":original.column,"name":original.name
                }),
            );
            row.function = original.name.or_else(|| node.function.clone());
            row.location = Some(format!(
                "{}:{}:{}",
                original.source,
                u64::from(original.line) + 1,
                u64::from(original.column) + 1
            ));
            result.push(row)?;
        }
        Ok(result)
    }

    /// accepts an embedded map only from bytes returned for the executed script.
    pub fn embedded(executed: &[u8], executed_sha256: &str, limits: Limits) -> Result<Self> {
        if executed.len() > limits.max_input_bytes || digest(executed) != executed_sha256 {
            return Err(Error("executed source identity mismatch".into()));
        }
        let source = std::str::from_utf8(executed).map_err(|e| Error(e.to_string()))?;
        let encoded = source
            .lines()
            .rev()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("//# sourceMappingURL=data:application/json;base64,")
            })
            .ok_or_else(|| Error("executed script has no supported embedded map".into()))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| Error(e.to_string()))?;
        let binding = Binding {
            generated_sha256: executed_sha256.into(),
            map_sha256: digest(&bytes),
        };
        Self::parse(executed, &bytes, &binding, limits)
    }

    pub fn parse(
        generated: &[u8],
        bytes: &[u8],
        expected: &Binding,
        limits: Limits,
    ) -> Result<Self> {
        if generated.len() > limits.max_input_bytes || bytes.len() > limits.max_input_bytes {
            return Err(Error("source-map input budget exceeded".into()));
        }
        if digest(generated) != expected.generated_sha256 || digest(bytes) != expected.map_sha256 {
            return Err(Error(
                "source map does not match its declared build binding".into(),
            ));
        }
        let mut header = crate::input(bytes, limits)?;
        let mappings = header
            .get("mappings")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error(
                    "expected a regular source map; indexed maps need separate interpretation"
                        .into(),
                )
            })?;
        if mappings.split([',', ';']).filter(|s| !s.is_empty()).count() > limits.max_records {
            return Err(Error("source-map segment budget exceeded".into()));
        }
        let map = sourcemap::SourceMap::from_slice(bytes).map_err(|e| Error(e.to_string()))?;
        let mut sources = vec![];
        for id in 0..map.get_source_count() {
            let name = map
                .get_source(id)
                .ok_or_else(|| Error("missing source-map source".into()))?
                .to_string();
            let content = map.get_source_contents(id).map(str::to_string);
            if name.len() > limits.max_string_bytes
                || content
                    .as_ref()
                    .is_some_and(|s| s.len() > limits.max_string_bytes)
            {
                return Err(Error(
                    "source-map source/content exceeds its record budget".into(),
                ));
            }
            sources.push(Source { id, name, content });
        }
        let segments = map
            .tokens()
            .map(|token| Segment {
                generated_line: token.get_dst_line(),
                generated_column: token.get_dst_col(),
                source: token.get_source().map(|_| token.get_src_id()),
                original_line: token.get_src_line(),
                original_column: token.get_src_col(),
                name: token.get_name().map(str::to_string),
            })
            .collect::<Vec<_>>();
        header.remove("mappings");
        header.remove("sources");
        header.remove("sourcesContent");
        let result = Self {
            binding: expected.clone(),
            sources,
            segments,
            header,
            limits,
        };
        result.validate()?;
        Ok(result)
    }

    fn validate(&self) -> Result<()> {
        if self.segments.len() + self.sources.len() + 1 > self.limits.max_records {
            return Err(Error("source-map record budget exceeded".into()));
        }
        for (id, source) in self.sources.iter().enumerate() {
            if source.id as usize != id {
                return Err(Error("source-map source ids are not contiguous".into()));
            }
            if source.name.len() > self.limits.max_string_bytes
                || source
                    .content
                    .as_ref()
                    .is_some_and(|v| v.len() > self.limits.max_string_bytes)
            {
                return Err(Error(
                    "source-map source/content exceeds its record budget".into(),
                ));
            }
        }
        for segment in &self.segments {
            if segment
                .source
                .is_some_and(|s| s as usize >= self.sources.len())
            {
                return Err(Error("source-map segment has unknown source".into()));
            }
        }
        if self.segments.windows(2).any(|w| {
            (w[0].generated_line, w[0].generated_column)
                > (w[1].generated_line, w[1].generated_column)
        }) {
            return Err(Error("source-map positions are unordered".into()));
        }
        Ok(())
    }

    pub fn lookup(&self, line: u32, column: u32) -> Option<OriginalPosition> {
        let index = self
            .segments
            .partition_point(|s| (s.generated_line, s.generated_column) <= (line, column));
        let segment = self.segments.get(index.checked_sub(1)?)?;
        if segment.generated_line != line {
            return None;
        }
        let source = self.sources.get(segment.source? as usize)?;
        Some(OriginalPosition {
            source: source.name.clone(),
            line: segment.original_line,
            column: segment.original_column,
            name: segment.name.clone(),
        })
    }

    pub fn records(&self, subject: &str) -> Result<Extraction> {
        let mut result = Extraction::new(
            self.limits,
            json!({"format":"source-map-v3","positions":"zero-based utf-16 columns and zero-based lines"}),
        );
        result.schemas = vec![
            schema(
                "sourcemap.header",
                "build binding and source-map metadata",
                &[
                    (
                        "binding",
                        FieldType::Object,
                        None,
                        "generated-script and map content digests",
                    ),
                    (
                        "metadata",
                        FieldType::Object,
                        None,
                        "source-native metadata excluding separately extracted sources/mappings",
                    ),
                ],
            ),
            schema(
                "sourcemap.source",
                "original source identity and available content",
                &[
                    ("id", FieldType::Unsigned, None, "source-map source index"),
                    (
                        "name",
                        FieldType::String,
                        None,
                        "source identity after sourceRoot resolution",
                    ),
                ],
            ),
            schema(
                "sourcemap.segment",
                "one mapping segment including explicit unmapped regions",
                &[
                    (
                        "generated_line",
                        FieldType::Unsigned,
                        None,
                        "zero-based generated line",
                    ),
                    (
                        "generated_column",
                        FieldType::Unsigned,
                        None,
                        "zero-based generated utf-16 column",
                    ),
                ],
            ),
        ];
        result.push(record(
            "header".into(),
            "sourcemap.header",
            subject,
            json!({"binding":self.binding,"metadata":self.header}),
        ))?;
        for source in &self.sources {
            result.push(record(
                format!("source:{}", source.id),
                "sourcemap.source",
                subject,
                serde_json::to_value(source).map_err(|e| Error(e.to_string()))?,
            ))?;
        }
        for (index, segment) in self.segments.iter().enumerate() {
            let mut segment = serde_json::to_value(segment).map_err(|e| Error(e.to_string()))?;
            segment["ordinal"] = json!(index);
            result.push(record(
                format!("segment:{index}"),
                "sourcemap.segment",
                subject,
                segment,
            ))?;
        }
        Ok(result)
    }

    pub fn from_records(records: &[Record]) -> Result<Self> {
        if records.len() > Limits::default().max_records {
            return Err(Error("source-map record budget exceeded".into()));
        }
        let header: Vec<_> = records
            .iter()
            .filter(|r| r.schema == "sourcemap.header")
            .collect();
        if header.len() != 1 {
            return Err(Error("expected one source-map header".into()));
        }
        let binding = serde_json::from_value(header[0].payload["binding"].clone())
            .map_err(|e| Error(e.to_string()))?;
        let metadata = header[0].payload["metadata"]
            .as_object()
            .cloned()
            .ok_or_else(|| Error("invalid source-map metadata".into()))?;
        let mut sources = vec![];
        let mut segments = vec![];
        for record in records {
            match record.schema.as_str() {
                "sourcemap.source" => sources.push(
                    serde_json::from_value::<Source>(record.payload.clone())
                        .map_err(|e| Error(e.to_string()))?,
                ),
                "sourcemap.segment" => {
                    let index = record.payload["ordinal"]
                        .as_u64()
                        .ok_or_else(|| Error("invalid mapping ordinal".into()))?;
                    let segment = serde_json::from_value::<Segment>(record.payload.clone())
                        .map_err(|e| Error(e.to_string()))?;
                    segments.push((index, segment));
                }
                _ => {}
            }
        }
        sources.sort_by_key(|s| s.id);
        segments.sort_by_key(|s| s.0);
        if segments
            .iter()
            .enumerate()
            .any(|(i, (id, _))| i as u64 != *id)
        {
            return Err(Error("missing or duplicate mapping ordinal".into()));
        }
        let result = Self {
            binding,
            sources,
            segments: segments.into_iter().map(|(_, s)| s).collect(),
            header: metadata,
            limits: Limits::default(),
        };
        result.validate()?;
        Ok(result)
    }
}

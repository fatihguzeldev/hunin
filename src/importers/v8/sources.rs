use crate::importers::SourceSnapshot;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::Path,
};

pub(in crate::importers) const MAX_TOTAL: u64 = 32 * 1024 * 1024;
fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn uri(reference: &str, base: &str) -> Result<String> {
    Ok(url::Url::parse(base)?.join(reference)?.into())
}
fn embedded_agrees(
    map: &sourcemap::DecodedMap,
    source_uri: &str,
    content: &str,
    base: &str,
) -> Option<bool> {
    match map {
        sourcemap::DecodedMap::Regular(map) => {
            for index in 0..map.get_source_count() {
                if map
                    .get_source(index)
                    .and_then(|s| uri(s, base).ok())
                    .as_deref()
                    == Some(source_uri)
                    && let Some(embedded) = map.get_source_contents(index)
                {
                    return Some(embedded == content);
                }
            }
            None
        }
        sourcemap::DecodedMap::Index(map) => map
            .sections()
            .filter_map(|s| s.get_sourcemap())
            .filter_map(|m| embedded_agrees(m, source_uri, content, base))
            .reduce(|a, b| a && b),
        sourcemap::DecodedMap::Hermes(_) => None,
    }
}
struct Script {
    data: Value,
    code_id: String,
    map: Option<sourcemap::DecodedMap>,
    originals: HashMap<String, (String, String)>,
}
#[derive(Default)]
pub(in crate::importers) struct Scripts {
    entries: HashMap<String, Vec<Script>>,
    snapshots: Vec<SourceSnapshot>,
    pub diagnostics: Vec<String>,
}
impl Scripts {
    pub fn load(path: &Path) -> Result<Self> {
        let mut result = Self::default();
        let mut snapshot_ids = HashSet::new();
        for file in super::input::script_paths(path)
            .into_iter()
            .filter(|p| p.is_file())
        {
            let input = fs::File::open(&file)?;
            anyhow::ensure!(
                input.metadata()?.len() <= MAX_TOTAL,
                "script metadata exceeds32MiB"
            );
            let mut bytes = Vec::new();
            input.take(MAX_TOTAL + 1).read_to_end(&mut bytes)?;
            anyhow::ensure!(
                bytes.len() as u64 <= MAX_TOTAL,
                "script metadata grew beyond32MiB"
            );
            for line in std::str::from_utf8(&bytes)?
                .lines()
                .filter(|line| !line.is_empty())
            {
                let data: Value = serde_json::from_str(line)?;
                let namespace = file
                    .file_name()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.strip_prefix("scripts-"))
                    .and_then(|s| s.strip_suffix(".jsonl"))
                    .and_then(|s| s.split_once('-'))
                    .context("script sidecar namespace missing")?;
                if data["pid"].as_u64().map(|v| v.to_string()).as_deref() != Some(namespace.0)
                    || data["tid"].as_str() != Some(namespace.1)
                {
                    result.diagnostics.push("Captured script producer/thread namespace disagrees with the profile sidecar; mapping was not selected".into());
                    continue;
                }
                let script_id = data["script_id"]
                    .as_str()
                    .context("captured script identity missing")?
                    .to_owned();
                let original_url = data["url"]
                    .as_str()
                    .context("captured script URL missing")?
                    .to_owned();
                let runtime = data["runtime"].as_str().unwrap_or("unknown");
                anyhow::ensure!(
                    data["schema_version"] == 1,
                    "unsupported script metadata schema"
                );
                let generated = data["generated_code"].as_str();
                let generated_sha = generated.map(hash);
                if let Some(sha) = &generated_sha
                    && data["generated_sha256"] != sha.as_str()
                {
                    result.diagnostics.push(format!("Captured script {script_id} content digest disagrees; source mapping was not applied"));
                    continue;
                }
                let code_id = match &generated_sha {
                    Some(sha) => format!("sha256:{sha}"),
                    None => data["code_id"]
                        .as_str()
                        .context("captured script has no code identity")?
                        .to_owned(),
                };
                let map_data = &data["source_map"];
                let map_sha = map_data["sha256"].as_str();
                let mut provenance = json!({"code_id":code_id,"script_id":script_id,"runtime":runtime,"script_uri":original_url,"generated_script_sha256":generated_sha,"map_sha256":map_sha,"build_mapping_verified":false,"compiler_mapping_proof":false});
                if let Some(content) = generated {
                    provenance["source_role"] = json!("runtime_observed_generated_script");
                    let source_uri = if runtime == "node" {
                        original_url.clone()
                    } else {
                        format!(
                            "hunin-generated://{}/script.js",
                            generated_sha.as_deref().unwrap()
                        )
                    };
                    if snapshot_ids.insert((source_uri.clone(), code_id.clone(), hash(content))) {
                        result.snapshots.push(SourceSnapshot {
                            uri: source_uri,
                            content: content.into(),
                            sha256: hash(content),
                            provenance: provenance.clone(),
                        });
                    }
                }
                let mut originals: HashMap<String, (String, String)> = HashMap::new();
                let mut pending_snapshots = Vec::new();
                let candidates = map_data["originals"]
                    .as_array()
                    .map(|v| v.iter().collect::<Vec<_>>())
                    .unwrap_or_default();
                for original in candidates.into_iter().chain(data.get("original")) {
                    let (Some(source_uri), Some(content)) =
                        (original["uri"].as_str(), original["content"].as_str())
                    else {
                        continue;
                    };
                    let sha = hash(content);
                    if original["sha256"] != sha {
                        result.diagnostics.push(format!("Captured original source {source_uri} digest disagrees; source was not selected"));
                        continue;
                    }
                    let basis = original["provenance"]
                        .as_str()
                        .unwrap_or("unknown")
                        .to_owned();
                    let mut prov = provenance.clone();
                    prov["source_role"] = json!("original_source");
                    prov["original_content_basis"] = json!(basis);
                    prov["source_position_basis"] = json!(if runtime == "node" {
                        "runtime_linked_source_map"
                    } else {
                        "runtime_reported"
                    });
                    originals.insert(source_uri.into(), (content.into(), basis));
                    pending_snapshots.push(SourceSnapshot {
                        uri: source_uri.into(),
                        content: content.into(),
                        sha256: sha,
                        provenance: prov,
                    });
                }
                let map = if runtime == "node" && !map_data.is_null() {
                    let decoded = (|| -> Result<sourcemap::DecodedMap> {
                        let content = map_data["content"]
                            .as_str()
                            .context("captured source-map content missing")?;
                        anyhow::ensure!(
                            Some(hash(content).as_str()) == map_sha,
                            "source-map digest disagrees"
                        );
                        let reference = data["source_map_url"]
                            .as_str()
                            .filter(|v| !v.is_empty())
                            .or_else(|| {
                                generated.and_then(|code| {
                                    code.rsplit("sourceMappingURL=")
                                        .next()
                                        .filter(|_| code.contains("sourceMappingURL="))
                                        .and_then(|s| s.split_whitespace().next())
                                })
                            })
                            .context("source map was not linked by the observed script")?;
                        let expected = if reference.starts_with("data:") {
                            original_url.clone()
                        } else {
                            uri(reference, &original_url)?
                        };
                        anyhow::ensure!(
                            map_data["uri"] == expected,
                            "source-map URI disagrees with the observed script reference"
                        );
                        let parsed: Value = serde_json::from_str(content)?;
                        let script_debug = generated.and_then(|code| {
                            code.lines()
                                .filter_map(|line| {
                                    line.trim()
                                        .strip_prefix("//#")
                                        .or_else(|| line.trim().strip_prefix("//@"))
                                })
                                .filter_map(|line| line.trim().strip_prefix("debugId"))
                                .filter_map(|rest| rest.trim().strip_prefix('='))
                                .filter_map(|value| value.split_whitespace().next())
                                .next_back()
                        });
                        if let (Some(script), Some(map)) =
                            (script_debug, parsed["debug_id"].as_str())
                        {
                            anyhow::ensure!(
                                script.eq_ignore_ascii_case(map),
                                "script and source-map debug IDs disagree"
                            );
                        }
                        if let Some(file) = parsed["file"].as_str().filter(|v| !v.is_empty()) {
                            anyhow::ensure!(
                                uri(file, &expected)? == original_url,
                                "source-map file field disagrees with the observed script URL"
                            );
                        }
                        Ok(sourcemap::decode_slice(content.as_bytes())?)
                    })();
                    match decoded {
                        Ok(map) => Some(map),
                        Err(error) => {
                            result.diagnostics.push(format!(
                                "Script {script_id} source map was not applied: {error:#}"
                            ));
                            None
                        }
                    }
                } else {
                    None
                };
                if runtime == "node" {
                    originals.retain(|source,(content,basis)| {
                        let agreement=map.as_ref().and_then(|map|embedded_agrees(map,source,content,map_data["uri"].as_str().unwrap_or("")));
                        let keep=map.is_some() && agreement!=Some(false) && (basis!="source_map_sourcesContent" || agreement==Some(true));
                        if !keep {result.diagnostics.push(format!("Captured source {source} was not selected: its map linkage or embedded content disagrees"));}
                        keep
                    });
                }
                for snapshot in pending_snapshots {
                    if originals.contains_key(&snapshot.uri)
                        && snapshot_ids.insert((
                            snapshot.uri.clone(),
                            code_id.clone(),
                            snapshot.sha256.clone(),
                        ))
                    {
                        result.snapshots.push(snapshot);
                    }
                }
                if let Some(error) = data["source_map_error"].as_str() {
                    result.diagnostics.push(format!(
                        "Script {script_id} source map was not captured: {error}"
                    ));
                }
                result.entries.entry(script_id).or_default().push(Script {
                    data,
                    code_id,
                    map,
                    originals,
                });
            }
        }
        for (id, entries) in &result.entries {
            if entries
                .iter()
                .map(|s| s.code_id.as_str())
                .collect::<HashSet<_>>()
                .len()
                > 1
            {
                result.diagnostics.push(format!("Script {id} has multiple captured code versions in one isolate; profile frames cannot select a version and remain unmapped"));
            }
        }
        Ok(result)
    }
    pub fn frame(&self, call: &Value, frame: &mut Value) -> Value {
        let Some(id) = call["scriptId"].as_str() else {
            return Value::Null;
        };
        let Some(entries) = self.entries.get(id) else {
            return Value::Null;
        };
        let unique: HashSet<_> = entries.iter().map(|s| s.code_id.as_str()).collect();
        if unique.len() != 1 {
            return json!({"script_id":id,"source_position_basis":"ambiguous_script_versions","diagnostic":"multiple captured code versions share this isolate script ID; mapping was not selected"});
        }
        let script = &entries[0];
        if script.data["url"] != call["url"] {
            return json!({"script_id":id,"source_position_basis":"script_url_mismatch"});
        }
        frame["code_id"] = json!(script.code_id);
        let mut proof = json!({"script_id":id,"code_id":script.code_id,"generated_script_sha256":script.data["generated_sha256"],"map_sha256":script.data["source_map"]["sha256"],"profile_position":{"file":call["url"],"line":call["lineNumber"].as_i64().map(|n|n+1),"column":call["columnNumber"]},"source_position_basis":script.data["profile_position_basis"],"build_mapping_verified":false});
        proof["original_content_available"] = json!(
            call["url"]
                .as_str()
                .is_some_and(|url| script.originals.contains_key(url))
        );
        let Some(map) = &script.map else { return proof };
        let (Some(line), Some(column)) =
            (call["lineNumber"].as_u64(), call["columnNumber"].as_u64())
        else {
            return proof;
        };
        let Some(token) = map
            .lookup_token(line as u32, column as u32)
            .filter(|t| t.has_source() && t.get_dst_line() as u64 == line)
        else {
            return proof;
        };
        let (Some(source), Some(base)) = (
            token.get_source(),
            script.data["source_map"]["uri"].as_str(),
        ) else {
            return proof;
        };
        let Ok(source_uri) = uri(source, base) else {
            return proof;
        };
        let Some((content, basis)) = script.originals.get(&source_uri) else {
            proof["source_position_basis"] = json!("source_map_original_content_unavailable");
            return proof;
        };
        if token
            .get_source_view()
            .is_some_and(|view| view.source() != content)
        {
            proof["source_position_basis"] = json!("source_map_original_content_mismatch");
            return proof;
        }
        if content
            .lines()
            .nth(token.get_src_line() as usize)
            .is_none_or(|line| token.get_src_col() as usize > line.encode_utf16().count())
        {
            proof["source_position_basis"] = json!("source_map_position_outside_retained_source");
            return proof;
        }
        frame["file"] = json!(source_uri);
        frame["line"] = json!(token.get_src_line() + 1);
        frame["column"] = json!(token.get_src_col());
        proof["source_position_basis"] = json!("runtime_linked_source_map");
        proof["original_content_basis"] = json!(basis);
        proof
    }
    pub fn into_snapshots(self) -> Vec<SourceSnapshot> {
        self.snapshots
    }
}

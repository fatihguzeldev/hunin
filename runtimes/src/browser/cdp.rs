use crate::{Extraction, Result};
use hunin_core::records::{FieldSchema, FieldType};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Default)]
enum Scope {
    #[default]
    Unbound,
    Context(String),
    Mixed,
}

// a CDP profiler session observes an isolate, which can include several page contexts.
pub(super) fn project_contexts(
    extraction: &mut Extraction,
    body: &Value,
    profile: &Value,
) -> Result<()> {
    let isolate = profile["isolateId"].as_str();
    let nodes = profile["profile"]["nodes"].as_array().unwrap();
    let mut contexts: BTreeMap<(&str, u64), Vec<&Value>> = BTreeMap::new();
    for context in body["executionContexts"].as_array().into_iter().flatten() {
        if let (Some(session), Some(id)) = (context["sessionId"].as_str(), context["id"].as_u64()) {
            contexts.entry((session, id)).or_default().push(context);
        }
    }
    let mut scripts: BTreeMap<(&str, &str), BTreeMap<String, Value>> = BTreeMap::new();
    if let Some(isolate) = isolate.filter(|id| !id.is_empty() && id.len() <= 128) {
        for script in body["scripts"].as_array().into_iter().flatten() {
            if script["isolateId"] != isolate {
                continue;
            }
            let (Some(script_id), Some(url), Some(session), Some(id)) = (
                script["scriptId"].as_str(),
                script["url"].as_str(),
                script["sessionId"].as_str(),
                script["executionContextId"].as_u64(),
            ) else {
                continue;
            };
            if let Some(contexts) = contexts.get(&(session, id))
                && contexts.len() == 1
            {
                let context = contexts[0];
                scripts.entry((script_id, url)).or_default().insert(
                    format!("isolate-{isolate}:context-{id}"),
                    json!({"isolate_id":isolate,"execution_context_id":id,"unique_id":context["uniqueId"],"frame_id":context["auxData"]["frameId"],"target_id":script["targetId"],"origin":"captured isolate + script id/url + execution context; not page CPU ownership"}),
                );
            }
        }
    }
    let mut bindings = BTreeMap::new();
    let mut parents = BTreeMap::new();
    for node in nodes {
        let id = node["id"].as_u64().unwrap();
        for child in node["children"].as_array().into_iter().flatten() {
            parents.insert(child.as_u64().unwrap(), id);
        }
        let frame = &node["callFrame"];
        if let Some(candidates) = scripts.get(&(
            frame["scriptId"].as_str().unwrap(),
            frame["url"].as_str().unwrap(),
        )) && candidates.len() == 1
        {
            let (subject, value) = candidates.iter().next().unwrap();
            bindings.insert(id, (subject.clone(), value.clone()));
        }
    }
    let context_bindings: BTreeMap<_, _> = scripts
        .values()
        .flat_map(|values| values.iter())
        .map(|(subject, binding)| (subject.clone(), binding.clone()))
        .collect();
    let mut stack_scopes: BTreeMap<u64, Scope> = BTreeMap::new();
    for node in nodes {
        let mut path = Vec::new();
        let mut at = node["id"].as_u64();
        while let Some(id) = at {
            if stack_scopes.contains_key(&id) {
                break;
            }
            path.push(id);
            at = parents.get(&id).copied();
        }
        let mut scope = at
            .and_then(|id| stack_scopes.get(&id))
            .cloned()
            .unwrap_or_default();
        for id in path.into_iter().rev() {
            if let Some((subject, _)) = bindings.get(&id) {
                scope = match scope {
                    Scope::Unbound => Scope::Context(subject.clone()),
                    Scope::Context(ref previous) if previous == subject => scope,
                    _ => Scope::Mixed,
                };
            }
            stack_scopes.insert(id, scope.clone());
        }
    }
    for schema in &mut extraction.schemas {
        schema.fields.insert(
            "browser_scope".into(),
            FieldSchema {
                kind: FieldType::Object,
                required: false,
                unit: None,
                meaning: "source-matched execution context, or explicitly unresolved/mixed profile scope; no page CPU ownership".into(),
            },
        );
    }
    let records = std::mem::take(&mut extraction.records);
    extraction.output_bytes = 0;
    for mut record in records {
        let node = match record.schema.as_str() {
            "v8.node" => record.payload["id"].as_u64(),
            "v8.sample" | "v8.position-tick" => record.payload["node"].as_u64(),
            _ => None,
        };
        if let Some(node) = node {
            let scope = if record.schema == "v8.sample" {
                stack_scopes.get(&node).cloned().unwrap_or_default()
            } else {
                bindings
                    .get(&node)
                    .map(|(subject, _)| Scope::Context(subject.clone()))
                    .unwrap_or_default()
            };
            let scope = match scope {
                Scope::Context(subject) => {
                    record.subject = subject.clone();
                    context_bindings[&subject].clone()
                }
                Scope::Unbound => {
                    json!({"status":"unbound","origin":"broad exported profiler scope; no selected-page inference"})
                }
                Scope::Mixed => {
                    json!({"status":"mixed stack contexts","origin":"broad exported profiler scope; individual node records retain captured context bindings"})
                }
            };
            record
                .payload
                .as_object_mut()
                .unwrap()
                .insert("browser_scope".into(), scope);
        }
        extraction.push(record)?;
    }
    extraction.metadata["subject_projection"] = json!(
        "profile headers/edges have broad profiler scope; function nodes use captured isolate/script/context identity; samples project only a single unambiguous stack context"
    );
    Ok(())
}

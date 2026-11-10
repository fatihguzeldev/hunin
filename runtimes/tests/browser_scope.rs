use hunin_core::{
    project::Project,
    query::{Query, QueryLimits},
    records::SourceDefinition,
};
use hunin_runtimes::{Limits, browser};
use serde_json::{Value, json};

fn frame(name: &str, script: &str, url: &str) -> Value {
    json!({"functionName":name,"scriptId":script,"url":url,"lineNumber":0,"columnNumber":0})
}
fn chromium() -> Value {
    let script = |id: &str, url: &str, session: &str, isolate: &str, context: u32| json!({"scriptId":id,"url":url,"sessionId":session,"isolateId":isolate,"executionContextId":context,"source":"function source() {}","sha256":hunin_runtimes::source_maps::digest(b"function source() {}")});
    json!({"kind":"chromium","selectedTargetId":"page","targets":[{"targetId":"page","sessionId":"p","type":"page","ready":true},{"targetId":"worker","sessionId":"w","type":"worker","ready":true}],"executionContexts":[{"sessionId":"p","id":2,"auxData":{"frameId":"page"}},{"sessionId":"o","id":3,"auxData":{"frameId":"other"}},{"sessionId":"w","id":1}],"scripts":[script("1","file:///main.js","p","A",2),script("2","file:///other.js","o","A",3),script("1","blob:worker","w","B",1)],"profiles":[{"targetId":"page","sessionId":"p","isolateId":"A","profile":{"startTime":0,"endTime":10,"nodes":[{"id":1,"callFrame":frame("(root)","0","") ,"children":[2,3]},{"id":2,"callFrame":frame("selectedMainWork","1","file:///main.js")},{"id":3,"callFrame":frame("unrelatedWork","2","file:///other.js")}],"samples":[2,3],"timeDeltas":[1,1]}},{"targetId":"worker","sessionId":"w","isolateId":"B","profile":{"startTime":0,"endTime":10,"nodes":[{"id":1,"callFrame":frame("(root)","0","") ,"children":[2]},{"id":2,"callFrame":frame("workerWork","1","blob:worker")}],"samples":[2],"timeDeltas":[1]}}]})
}
fn gecko() -> Value {
    json!({"kind":"firefox","scopeBindings":{"realms":{"realms":[{"type":"dedicated-worker","origin":"blob:worker"}]}},"profile":{"meta":{"version":36,"sampleUnits":{"time":"ms"}},"pages":[{"innerWindowID":11,"url":"file:///main.html"},{"innerWindowID":22,"url":"file:///other.html"}],"threads":[{"pid":7,"tid":7,"name":"GeckoMain","stringTable":["selectedMainWork (file:///main.js:1:1)","unrelatedWork (file:///other.js:1:1)"],"frameTable":{"schema":{"location":0,"innerWindowID":1},"data":[[0,11],[1,22]]},"stackTable":{"schema":{"prefix":0,"frame":1},"data":[[null,0],[null,1],[0,1]]},"samples":{"schema":{"stack":0,"time":1},"data":[[0,1],[1,2],[2,3]]}},{"pid":7,"tid":9,"name":"DOM Worker","stringTable":["workerWork (blob:worker:1:1)"],"frameTable":{"schema":{"location":0,"innerWindowID":1},"data":[[0,11]]},"stackTable":{"schema":{"prefix":0,"frame":1},"data":[[null,0]]},"samples":{"schema":{"stack":0,"time":1},"data":[[0,1]]}}],"processes":[]}})
}
fn subjects(extracted: &browser::BrowserExtraction) -> [String; 3] {
    ["selectedMainWork", "workerWork", "unrelatedWork"].map(|function| {
        extracted
            .streams
            .iter()
            .flat_map(|s| &s.extraction.records)
            .find(|r| {
                r.function.as_deref() == Some(function)
                    && (r.schema == "v8.node"
                        || (r.schema == "gecko.table-row" && r.payload["table"] == "frameTable"))
            })
            .unwrap_or_else(|| panic!("missing {function} frame"))
            .subject
            .clone()
    })
}
#[test]
fn shared_isolate_and_shared_gecko_thread_do_not_merge_window_subjects() {
    for body in [chromium(), gecko()] {
        let result = browser::extract(&body, "collection", Limits::default(), None).unwrap();
        let [selected, worker, other] = subjects(&result);
        assert_ne!(selected, worker);
        assert_ne!(selected, other);
        assert_ne!(worker, other);
        assert!(
            result
                .streams
                .iter()
                .flat_map(|s| &s.extraction.records)
                .filter(|r| r.subject == selected)
                .all(|r| r.function.as_deref() != Some("unrelatedWork"))
        );
        assert!(result.collection.capture_complete);
        let mut expected = body.as_object().unwrap().clone();
        for key in ["profiles", "profile", "scripts"] {
            expected.remove(key);
        }
        assert_eq!(
            result.collection.records[0].payload["metadata"],
            Value::Object(expected)
        );
    }
}
#[test]
fn mixed_window_stacks_and_missing_worker_sources_remain_explicit() {
    let body = gecko();
    let result = browser::extract(&body, "collection", Limits::default(), None).unwrap();
    let mixed = result.streams[0]
        .extraction
        .records
        .iter()
        .find(|r| r.payload["table"] == "samples" && r.payload["index"] == 2)
        .unwrap();
    assert!(mixed.subject.ends_with("thread-0"));
    assert_eq!(mixed.payload["scope"]["window_ids"], json!([11, 22]));
    let mut body = chromium();
    body["profiles"].as_array_mut().unwrap().pop();
    let result = browser::extract(&body, "collection", Limits::default(), None).unwrap();
    assert!(!result.collection.capture_complete);
    assert!(
        result
            .collection
            .warnings
            .iter()
            .any(|w| w.contains("worker"))
    );
    assert!(
        result.streams[0]
            .extraction
            .records
            .iter()
            .any(|r| r.function.as_deref() == Some("selectedMainWork"))
    );
    let mut body = gecko();
    body["profile"]["threads"].as_array_mut().unwrap().pop();
    let result = browser::extract(&body, "collection", Limits::default(), None).unwrap();
    assert!(!result.collection.capture_complete);
    assert!(
        result
            .collection
            .warnings
            .iter()
            .any(|w| w.contains("worker"))
    );
    let mut body = chromium();
    body["profiles"][0]["profile"]["nodes"][0]["children"] = json!([2]);
    body["profiles"][0]["profile"]["nodes"][1]["children"] = json!([3]);
    let result = browser::extract(&body, "collection", Limits::default(), None).unwrap();
    let mixed = result.streams[0]
        .extraction
        .records
        .iter()
        .find(|r| r.schema == "v8.sample" && r.payload["node"] == 3)
        .unwrap();
    assert_eq!(mixed.subject, "profile-page");
    assert_eq!(
        mixed.payload["browser_scope"]["status"],
        "mixed stack contexts"
    );
}

#[test]
fn missing_or_ambiguous_context_bindings_stay_broad_and_projected_bytes_are_bounded() {
    let mut body = chromium();
    body["executionContexts"]
        .as_array_mut()
        .unwrap()
        .push(json!({"sessionId":"p","id":2,"uniqueId":"different-lifetime"}));
    let result = browser::extract(&body, "collection", Limits::default(), None).unwrap();
    let selected = result.streams[0]
        .extraction
        .records
        .iter()
        .find(|r| r.schema == "v8.node" && r.function.as_deref() == Some("selectedMainWork"))
        .unwrap();
    assert_eq!(selected.subject, "profile-page");
    assert_eq!(selected.payload["browser_scope"]["status"], "unbound");
    let body = chromium();
    let result = browser::extract(&body, "collection", Limits::default(), None).unwrap();
    let bytes: usize = result
        .collection
        .records
        .iter()
        .chain(result.streams.iter().flat_map(|s| &s.extraction.records))
        .map(|r| serde_json::to_vec(r).unwrap().len())
        .sum();
    assert!(
        browser::extract(
            &body,
            "collection",
            Limits {
                max_output_bytes: bytes - 1,
                ..Limits::default()
            },
            None
        )
        .is_err()
    );
    let mut body = gecko();
    body["profile"]["threads"][0]["frameTable"]["data"][0][1] = json!(u64::MAX);
    let result = browser::extract(&body, "collection", Limits::default(), None).unwrap();
    let selected = result.streams[0]
        .extraction
        .records
        .iter()
        .find(|r| {
            r.payload["table"] == "frameTable" && r.function.as_deref() == Some("selectedMainWork")
        })
        .unwrap();
    assert!(selected.subject.ends_with("thread-0"));
    assert_eq!(selected.payload["scope"]["raw_inner_window_id"], u64::MAX);
    assert!(
        selected.payload["scope"]["binding"]
            .as_str()
            .unwrap()
            .contains("unsupported")
    );
}

#[test]
#[ignore = "requires an actual headless scope recording/report from the three independently checked work paths"]
fn actual_browser_contexts_workers_and_missing_source_control_survive_reopen() {
    let input = tempfile::tempdir().unwrap();
    let raw = input.path().join("raw.json");
    std::fs::copy(
        std::env::var_os("HUNIN_BROWSER_SCOPE_RECORDING").unwrap(),
        &raw,
    )
    .unwrap();
    let body: Value = serde_json::from_slice(&std::fs::read(&raw).unwrap()).unwrap();
    let report: Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("HUNIN_BROWSER_SCOPE_REPORT").unwrap()).unwrap(),
    )
    .unwrap();
    let count = 50_000_000u64;
    let expected = count * (count - 1) / 2 % (1u64 << 32);
    assert_eq!(report["outcome"], "completed");
    for key in ["selected", "worker", "unrelated"] {
        assert_eq!(report["output"][key], expected);
    }
    let result = browser::extract(&body, "collection", Limits::default(), Some(report)).unwrap();
    assert!(
        result.collection.capture_complete,
        "{:?}",
        result.collection.warnings
    );
    let [selected, worker, other] = subjects(&result);
    assert_ne!(selected, other);
    assert_ne!(selected, worker);
    assert_ne!(worker, other);
    if body["kind"] == "chromium" {
        assert!(
            result
                .collection
                .records
                .iter()
                .any(|r| r.schema == "browser.execution-context")
        );
        let primary = body["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["targetId"] == body["selectedTargetId"])
            .unwrap();
        assert!(
            primary["profile"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["callFrame"]["functionName"] == "unrelatedWork"),
            "the control must actually share the profiled isolate"
        );
        assert!(body["scripts"].as_array().unwrap().iter().any(|s| {
            s["url"]
                .as_str()
                .is_some_and(|url| url.starts_with("blob:"))
        }));
    } else {
        assert!(
            result
                .collection
                .records
                .iter()
                .any(|r| r.schema == "browser.realm")
        );
    }
    let data = tempfile::tempdir().unwrap();
    let p = Project::create(data.path(), "p").unwrap();
    let i = p
        .create_investigation("i", "browser scope", json!({}))
        .unwrap();
    let run = p.create_run(&i, "r", json!({})).unwrap();
    let mut kept = vec![];
    let mut all = vec![("collection".to_string(), result.collection)];
    all.extend(result.streams.into_iter().map(|s| (s.key, s.extraction)));
    for (id, extracted) in all {
        let source = p
            .register_source(
                &run,
                SourceDefinition {
                    id,
                    target: None,
                    schemas: extracted.schemas,
                    clocks: extracted.clocks,
                    interpretation: if body["kind"] == "chromium" {
                        "browser-v8-v2"
                    } else {
                        "browser-gecko36-v2"
                    }
                    .into(),
                    metadata: extracted.metadata,
                },
            )
            .unwrap();
        for batch in extracted.records.chunks(64) {
            source.append(batch).unwrap();
        }
        source
            .close(json!({"complete":extracted.capture_complete,"warnings":extracted.warnings}))
            .unwrap();
        for record in extracted
            .records
            .iter()
            .filter(|r| {
                r.function.as_deref().is_some_and(|f| {
                    ["selectedMainWork", "workerWork", "unrelatedWork"].contains(&f)
                })
            })
            .take(4)
        {
            kept.push((source.record_ref(&record.id), record.clone()));
        }
    }
    std::fs::remove_file(raw).unwrap();
    drop(p);
    let p = Project::open(data.path(), "p").unwrap();
    for (reference, record) in kept {
        assert_eq!(p.record(&reference, None).unwrap(), record);
    }
    let mut q = Query::new(run);
    q.subject = Some(selected);
    q.function = Some("selectedMainWork".into());
    assert!(
        !p.query(&q, QueryLimits::default(), None)
            .unwrap()
            .records
            .is_empty()
    );
    q.function = Some("unrelatedWork".into());
    assert!(
        p.query(&q, QueryLimits::default(), None)
            .unwrap()
            .records
            .is_empty()
    );
    q.subject = Some(other);
    assert!(
        !p.query(&q, QueryLimits::default(), None)
            .unwrap()
            .records
            .is_empty()
    );
    let mut missing = body.clone();
    if body["kind"] == "chromium" {
        missing["profiles"]
            .as_array_mut()
            .unwrap()
            .retain(|p| p["type"] != "worker");
    } else {
        let mut pending = vec![&mut missing["profile"]];
        while let Some(process) = pending.pop() {
            process["threads"].as_array_mut().unwrap().retain(|t| {
                !t["stringTable"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s.as_str().is_some_and(|s| s.contains("workerWork")))
            });
            if let Some(children) = process["processes"].as_array_mut() {
                pending.extend(children.iter_mut());
            }
        }
    }
    let incomplete = browser::extract(&missing, "collection", Limits::default(), None).unwrap();
    assert!(!incomplete.collection.capture_complete);
    assert!(
        incomplete
            .collection
            .warnings
            .iter()
            .any(|w| w.contains("worker"))
    );
    assert!(
        incomplete
            .streams
            .iter()
            .flat_map(|s| &s.extraction.records)
            .any(|r| r.function.as_deref() == Some("selectedMainWork"))
    );
}

#[test]
#[ignore = "requires an actual headless Chromium capture whose worker exited after completing its work"]
fn terminated_worker_retains_main_evidence_and_an_explicit_durable_gap() {
    let input = tempfile::tempdir().unwrap();
    let raw = input.path().join("raw.json");
    std::fs::copy(
        std::env::var_os("HUNIN_BROWSER_SCOPE_RECORDING").unwrap(),
        &raw,
    )
    .unwrap();
    let body: Value = serde_json::from_slice(&std::fs::read(&raw).unwrap()).unwrap();
    assert_eq!(body["kind"], "chromium");
    assert!(
        body["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|target| target["type"] == "worker" && target["ready"] == true)
    );
    assert!(!body["sourceFailures"].as_array().unwrap().is_empty());
    let result = browser::extract(&body, "collection", Limits::default(), None).unwrap();
    assert!(!result.collection.capture_complete);
    assert!(
        result
            .collection
            .warnings
            .iter()
            .any(|w| w.contains("worker"))
    );
    let data = tempfile::tempdir().unwrap();
    let p = Project::create(data.path(), "p").unwrap();
    let i = p
        .create_investigation("i", "worker source exit", json!({}))
        .unwrap();
    let run = p.create_run(&i, "r", json!({})).unwrap();
    let mut known = None;
    let mut all = vec![("collection".to_string(), result.collection)];
    all.extend(result.streams.into_iter().map(|s| (s.key, s.extraction)));
    for (id, extracted) in all {
        let source = p
            .register_source(
                &run,
                SourceDefinition {
                    id,
                    target: None,
                    schemas: extracted.schemas,
                    clocks: extracted.clocks,
                    interpretation: "browser-v8-v2".into(),
                    metadata: extracted.metadata,
                },
            )
            .unwrap();
        for batch in extracted.records.chunks(64) {
            source.append(batch).unwrap();
        }
        source
            .close(json!({"complete":extracted.capture_complete,"warnings":extracted.warnings}))
            .unwrap();
        if let Some(record) = extracted
            .records
            .iter()
            .find(|r| r.schema == "v8.node" && r.function.as_deref() == Some("selectedMainWork"))
        {
            known = Some((source.record_ref(&record.id), record.clone()));
        }
    }
    let (reference, record) = known.expect("live main profile survives the worker source exit");
    std::fs::remove_file(raw).unwrap();
    drop(p);
    let p = Project::open(data.path(), "p").unwrap();
    assert_eq!(p.record(&reference, None).unwrap(), record);
    let mut query = Query::new(run);
    query.schema = Some("browser.collection".into());
    let rows = p
        .query(&query, QueryLimits::default(), None)
        .unwrap()
        .records;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        p.record(&rows[0].reference, None).unwrap().payload["coverage"]["complete"],
        false
    );
}

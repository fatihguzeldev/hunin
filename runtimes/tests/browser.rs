use hunin_core::{
    project::Project,
    query::{Query, QueryLimits},
    records::SourceDefinition,
};
use hunin_runtimes::{Limits, browser};
use serde_json::{Value, json};

fn synthetic_gecko() -> Value {
    json!({"kind":"firefox","profile":{"meta":{"version":36,"sampleUnits":{"time":"ms","threadCPUDelta":"µs"}},"threads":[{"pid":7,"tid":9,"name":"GeckoMain","stringTable":["resize (file:///preview.js:3:2)[1]"],"frameTable":{"schema":{"location":0},"data":[[0]]},"stackTable":{"schema":{"prefix":0,"frame":1},"data":[[null,0]]},"samples":{"schema":{"stack":0,"time":1},"data":[[0,1.2345]]},"markers":{"schema":{"name":0},"data":[]}}],"processes":[]}})
}

#[test]
fn chromium_mapping_cannot_borrow_debugger_bytes_from_another_profiler_session() {
    use hunin_runtimes::source_maps::{Binding, BoundMap, digest};
    let a = b"function resize() { return 1; }";
    let b = b"function resize() { return 2; }";
    let map_bytes=serde_json::to_vec(&json!({"version":3,"file":"generated.js","sources":["original.ts"],"sourcesContent":["function resize():number { return 1; }"],"names":[],"mappings":"AAAA"})).unwrap();
    let map = BoundMap::parse(
        a,
        &map_bytes,
        &Binding {
            generated_sha256: digest(a),
            map_sha256: digest(&map_bytes),
        },
        Limits::default(),
    )
    .unwrap();
    let profile = json!({"startTime":0,"endTime":10,"nodes":[{"id":1,"callFrame":{"functionName":"resize","scriptId":"7","url":"file:///generated.js","lineNumber":0,"columnNumber":0}}],"samples":[1],"timeDeltas":[1]});
    let body = json!({"kind":"chromium","profiles":[{"targetId":"page","sessionId":"page-session","profile":profile},{"targetId":"worker","sessionId":"worker-session","profile":profile}],"scripts":[{"scriptId":"7","url":"file:///generated.js","sessionId":"page-session","source":std::str::from_utf8(a).unwrap(),"sha256":digest(a)},{"scriptId":"7","url":"file:///generated.js","sessionId":"worker-session","source":std::str::from_utf8(b).unwrap(),"sha256":digest(b)}]});
    let extracted = browser::extract(&body, "b", Limits::default(), None).unwrap();
    let projected = browser::maps::project_chromium(
        &map,
        &extracted,
        &extracted.streams[0],
        "file:///generated.js",
        Limits::default(),
    )
    .unwrap();
    assert_eq!(projected.records.len(), 1);
    assert_eq!(
        projected.records[0].subject,
        extracted.streams[0]
            .extraction
            .records
            .iter()
            .find(|r| r.schema == "v8.node")
            .unwrap()
            .subject
    );
    assert!(
        browser::maps::project_chromium(
            &map,
            &extracted,
            &extracted.streams[1],
            "file:///generated.js",
            Limits::default()
        )
        .is_err()
    );
}

#[test]
fn gecko_source_mapping_cannot_upgrade_an_unsupported_raw_window_identity() {
    use hunin_runtimes::source_maps::{Binding, BoundMap, digest};
    let generated = b"function resize(){return 1}";
    let bytes = serde_json::to_vec(
        &json!({"version":3,"sources":["original.ts"],"names":[],"mappings":"AAAA"}),
    )
    .unwrap();
    let map = BoundMap::parse(
        generated,
        &bytes,
        &Binding {
            generated_sha256: digest(generated),
            map_sha256: digest(&bytes),
        },
        Limits::default(),
    )
    .unwrap();
    for window in [
        json!(11),
        json!(0),
        json!(u64::MAX),
        json!(9_007_199_254_740_992u64),
        json!(-1),
        json!(11.0),
    ] {
        let body = json!({"kind":"firefox","scopeBindings":{"realms":{"realms":[]}},"scripts":[{"scriptId":"file:///generated.js","url":"file:///generated.js","context":"page","page_url":"file:///main.html","source":std::str::from_utf8(generated).unwrap(),"sha256":digest(generated)}],"profile":{"meta":{"version":36,"sampleUnits":{"time":"ms"}},"pages":[{"innerWindowID":window,"url":"file:///main.html"}],"threads":[{"pid":7,"tid":7,"name":"GeckoMain","stringTable":["resize (file:///generated.js:1:1)"],"frameTable":{"schema":{"location":0,"innerWindowID":1},"data":[[0,window]]},"stackTable":{"schema":{"prefix":0,"frame":1},"data":[[null,0]]},"samples":{"schema":{"stack":0,"time":1},"data":[[0,1]]}}],"processes":[]}});
        let extracted = browser::extract(&body, "collection", Limits::default(), None).unwrap();
        let mapped = browser::maps::project_gecko(
            &map,
            &extracted,
            "file:///generated.js",
            "selected-page",
            Limits::default(),
        );
        assert_eq!(mapped.is_ok(), window == json!(11), "{window}");
    }
}

#[test]
fn process_local_tables_clocks_and_unknown_metadata_remain_structured() {
    let mut input = synthetic_gecko();
    input["profile"]["customOptional"] = json!({"counter":u64::MAX});
    let extracted = browser::extract(&input, "browser", Limits::default(), None).unwrap();
    assert_eq!(extracted.streams.len(), 1);
    let stream = &extracted.streams[0].extraction;
    let sample = stream
        .records
        .iter()
        .find(|r| r.schema == "gecko.table-row" && r.payload["table"] == "samples")
        .unwrap();
    assert_eq!(sample.function.as_deref(), Some("resize"));
    assert_eq!(sample.time.as_ref().unwrap().start, 1234);
    assert_eq!(sample.payload["values"][1], json!(1.2345));
    assert!(stream.records.iter().any(|r| r.schema == "gecko.metadata"
        && r.payload["field"] == "customOptional"
        && r.payload["value"]["data"]["counter"] == u64::MAX));
}

#[test]
fn wrong_units_unknown_versions_bad_references_and_cycles_fail() {
    for mode in 0..4 {
        let mut input = synthetic_gecko();
        match mode {
            0 => input["profile"]["meta"]["version"] = json!(37),
            1 => input["profile"]["meta"]["sampleUnits"]["time"] = json!("ns"),
            2 => input["profile"]["threads"][0]["samples"]["data"][0][0] = json!(99),
            _ => input["profile"]["threads"][0]["stackTable"]["data"][0][0] = json!(0),
        }
        assert!(
            browser::extract(&input, "browser", Limits::default(), None).is_err(),
            "mode{mode}"
        );
    }
    let input = synthetic_gecko();
    let first = browser::extract(&input, "browser", Limits::default(), None).unwrap();
    let mut input = synthetic_gecko();
    input["profile"]["processes"] = json!([input["profile"].clone()]);
    let limit = Limits {
        max_records: first.collection.records.len() + first.streams[0].extraction.records.len() + 1,
        ..Limits::default()
    };
    assert!(
        browser::extract(&input, "browser", limit, None).is_err(),
        "the record budget is shared across process streams"
    );
}

#[test]
#[ignore = "requires HUNIN_NODE; does not launch a browser"]
fn failed_browser_start_closes_control_input_and_exits_with_diagnostics() {
    use hunin_core::execution::PreparedProcess;
    let area = tempfile::tempdir().unwrap();
    let helper = area.path().join("browser.cjs");
    std::fs::write(&helper, include_str!("../helpers/browser.cjs")).unwrap();
    let mut process = PreparedProcess::spawn(&hunin_core::execution::PreparedLaunch {
        program: std::env::var_os("HUNIN_NODE").unwrap().into(),
        args: vec![
            helper.to_string_lossy().into_owned(),
            "unknown-browser".into(),
            "/absent/browser".into(),
            String::new(),
            helper.to_string_lossy().into_owned(),
            area.path().to_string_lossy().into_owned(),
        ],
        directory: area.path().to_owned(),
        environment: Default::default(),
        stdout_limit: 4096,
    })
    .unwrap();
    let start = std::time::Instant::now();
    let error = process
        .await_ready(std::time::Duration::from_secs(3))
        .unwrap_err();
    assert!(error.to_string().contains("unsupported browser collector"));
    let outcome = process
        .cancel(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
        )
        .unwrap();
    assert_eq!(outcome.exit_code, Some(1));
    assert!(!outcome.cleanup_pending);
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
}

#[test]
#[ignore = "requires actual JS/TS browser recording, generated script and build-bound map"]
fn browser_typescript_projection_requires_debugger_bytes_and_survives_source_removal() {
    use hunin_runtimes::source_maps::{Binding, BoundMap, digest};
    let area = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for (name, variable) in [
        ("raw.json", "HUNIN_BROWSER_RECORDING"),
        ("generated.js", "HUNIN_BROWSER_GENERATED"),
        ("generated.js.map", "HUNIN_BROWSER_MAP"),
    ] {
        let copy = area.path().join(name);
        std::fs::copy(std::env::var_os(variable).unwrap(), &copy).unwrap();
        paths.push(copy);
    }
    let raw: Value = serde_json::from_slice(&std::fs::read(&paths[0]).unwrap()).unwrap();
    let js = std::fs::read(&paths[1]).unwrap();
    let map_bytes = std::fs::read(&paths[2]).unwrap();
    let map = BoundMap::parse(
        &js,
        &map_bytes,
        &Binding {
            generated_sha256: digest(&js),
            map_sha256: digest(&map_bytes),
        },
        Limits::default(),
    )
    .unwrap();
    let browser = browser::extract(&raw, "browser", Limits::default(), None).unwrap();
    let script = raw["scripts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["sha256"] == digest(&js) || s["binding"]["original_sha256"] == digest(&js))
        .unwrap();
    let mut projections = Vec::new();
    if raw["kind"] == "chromium" {
        for stream in &browser.streams {
            if stream.extraction.metadata["session_id"] != script["sessionId"] {
                continue;
            }
            projections.push(
                browser::maps::project_chromium(
                    &map,
                    &browser,
                    stream,
                    script["url"].as_str().unwrap(),
                    Limits::default(),
                )
                .unwrap(),
            );
        }
        assert!(
            map.project_v8(
                script["scriptId"].as_str().unwrap(),
                "stale-build",
                &browser.streams[0].extraction.records,
                "prepared-page"
            )
            .is_err()
        );
    } else if raw["kind"] == "safari" {
        assert!(
            browser.streams[0].extraction.records.iter().any(
                |r| r.schema == "safari.call" && r.function.as_deref() == Some("resizePreview")
            ),
            "declared functions alone do not prove execution"
        );
        projections.push(
            browser::maps::project_safari(
                &map,
                &browser.streams[0],
                "prepared-page",
                Limits::default(),
            )
            .unwrap(),
        );
    } else {
        projections.push(
            browser::maps::project_gecko(
                &map,
                &browser,
                script["url"].as_str().unwrap(),
                "prepared-page",
                Limits::default(),
            )
            .unwrap(),
        );
        let mut shared_creator = raw.clone();
        fn add_same_url_worker(profile: &mut Value) -> Option<usize> {
            if let Some(threads) = profile["threads"].as_array_mut()
                && let Some(mut worker) = threads
                    .iter()
                    .find(|t| {
                        t["name"] == "GeckoMain"
                            && t["stringTable"].as_array().is_some_and(|s| {
                                s.iter().any(|v| {
                                    v.as_str().is_some_and(|s| s.contains("resizePreview"))
                                })
                            })
                    })
                    .cloned()
            {
                worker["name"] = json!("DOM Worker");
                worker["tid"] = json!(999_999_999);
                let index = threads.len();
                threads.push(worker);
                return Some(index);
            }
            profile["processes"]
                .as_array_mut()
                .and_then(|v| v.iter_mut().find_map(add_same_url_worker))
        }
        let worker_index = add_same_url_worker(&mut shared_creator["profile"]).unwrap();
        let shared_creator =
            browser::extract(&shared_creator, "browser", Limits::default(), None).unwrap();
        let projected = browser::maps::project_gecko(
            &map,
            &shared_creator,
            script["url"].as_str().unwrap(),
            "prepared-page",
            Limits::default(),
        )
        .unwrap();
        assert_eq!(
            projected.records.len(),
            projections.last().unwrap().records.len(),
            "creator window and source URL do not prove the worker executed selected-page bytes"
        );
        assert!(projected.records.iter().all(|r| {
            !r.payload["generated_record"]
                .as_str()
                .unwrap()
                .starts_with(&format!("thread-{worker_index}:"))
        }));
        let mut ambiguous = raw.clone();
        fn duplicate_page(profile: &mut Value) -> bool {
            if let Some(pages) = profile["pages"].as_array_mut()
                && let Some(page) = pages
                    .iter()
                    .find(|p| {
                        p["url"]
                            .as_str()
                            .is_some_and(|u| u.ends_with("browser-assets.html"))
                    })
                    .cloned()
            {
                pages.push(page);
                return true;
            }
            profile["processes"]
                .as_array_mut()
                .is_some_and(|v| v.iter_mut().any(duplicate_page))
        }
        assert!(duplicate_page(&mut ambiguous["profile"]));
        let ambiguous = browser::extract(&ambiguous, "browser", Limits::default(), None).unwrap();
        assert!(
            browser::maps::project_gecko(
                &map,
                &ambiguous,
                script["url"].as_str().unwrap(),
                "prepared-page",
                Limits::default()
            )
            .is_err()
        );
    }
    assert!(
        projections
            .iter()
            .any(|e| e
                .records
                .iter()
                .any(|r| r.function.as_deref() == Some("resizePreview")
                    && r.payload["line"] == 1
                    && r.location
                        .as_ref()
                        .is_some_and(|p| p.contains("browser-assets.ts:2:")))),
        "manually selected fixture declaration is TS line2"
    );
    let data = tempfile::tempdir().unwrap();
    let p = Project::create(data.path(), "p").unwrap();
    let i = p
        .create_investigation("i", "browser typescript", json!({}))
        .unwrap();
    let run = p.create_run(&i, "r", json!({})).unwrap();
    projections.push(map.records("build-map").unwrap());
    for (index, extracted) in projections.into_iter().enumerate() {
        let source = p
            .register_source(
                &run,
                SourceDefinition {
                    id: format!("map-{index}"),
                    target: None,
                    schemas: extracted.schemas,
                    clocks: extracted.clocks,
                    interpretation: "browser-map-v1".into(),
                    metadata: extracted.metadata,
                },
            )
            .unwrap();
        for (n, batch) in extracted.records.chunks(128).enumerate() {
            source.append_batch(&n.to_string(), batch).unwrap();
        }
        source.close(json!({"complete":true})).unwrap();
    }
    for file in paths {
        std::fs::remove_file(file).unwrap();
    }
    drop(p);
    let p = Project::open(data.path(), "p").unwrap();
    let mut q = Query::new(run);
    q.function = Some("resizePreview".into());
    assert!(
        !p.query(&q, QueryLimits::default(), None)
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
#[ignore = "requires HUNIN_BROWSER_RECORDING from an actual browser collector"]
fn actual_browser_profile_retains_function_source_and_all_typed_rows_after_input_removal() {
    let path = std::path::PathBuf::from(std::env::var("HUNIN_BROWSER_RECORDING").unwrap());
    let input_area = tempfile::tempdir().unwrap();
    let original = input_area.path().join("actual-input.json");
    std::fs::copy(path, &original).unwrap();
    let bytes = std::fs::read(&original).unwrap();
    let profile: Value = serde_json::from_slice(&bytes).unwrap();
    let extracted = browser::extract(&profile, "browser", Limits::default(), None).unwrap();
    assert!(
        extracted
            .streams
            .iter()
            .any(
                |s| s.extraction.records.iter().any(|r| r.function.as_deref()
                    == Some("resizePreview")
                    && r.location
                        .as_ref()
                        .is_some_and(|p| p.contains("browser-preview.js")))
            )
    );
    let data = tempfile::tempdir().unwrap();
    let p = Project::create(data.path(), "p").unwrap();
    let i = p.create_investigation("i", "browser", json!({})).unwrap();
    let run = p.create_run(&i, "r", json!({})).unwrap();
    let mut expected = Vec::new();
    for stream in extracted.streams {
        let source = p
            .register_source(
                &run,
                SourceDefinition {
                    id: stream.key.clone(),
                    target: None,
                    schemas: stream.extraction.schemas,
                    clocks: stream.extraction.clocks,
                    interpretation: format!("browser-{:?}", profile["kind"]),
                    metadata: stream.extraction.metadata,
                },
            )
            .unwrap();
        for (index, batch) in stream.extraction.records.chunks(128).enumerate() {
            source
                .append_batch(&format!("batch{index}"), batch)
                .unwrap();
        }
        source.close(json!({"complete":true})).unwrap();
        expected.push((stream.key, stream.extraction.records.len()));
    }
    std::fs::remove_file(original).unwrap();
    drop(p);
    let p = Project::open(data.path(), "p").unwrap();
    for (source, count) in expected {
        let mut query = Query::new(run.clone());
        query.source = Some(source);
        let mut cursor = None;
        let mut actual = 0;
        loop {
            let page = p
                .query(&query, QueryLimits::default(), cursor.as_ref())
                .unwrap();
            actual += page.records.len();
            match page.continuation {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(actual, count);
    }
}

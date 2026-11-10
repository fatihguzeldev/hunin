use hunin_runtimes::{Limits, python, v8};
use serde_json::json;

#[test]
fn v8_keeps_samples_edges_positions_and_unknown_source_fields() {
    let input = json!({
        "startTime": 1000, "endTime": 2000, "extraHeader": "retained",
        "nodes": [
            {"id":1,"callFrame":{"functionName":"(root)","scriptId":"0","url":"","lineNumber":-1,"columnNumber":-1},"children":[2]},
            {"id":2,"callFrame":{"functionName":"resize","scriptId":"7","url":"file:///image.js","lineNumber":12,"columnNumber":3},"hitCount":2,"positionTicks":[{"line":13,"ticks":2}],"futureField":true}
        ], "samples":[2,2], "timeDeltas":[100,200]
    });
    let extracted = v8::extract(
        &serde_json::to_vec(&input).unwrap(),
        "process:one",
        Limits::default(),
    )
    .unwrap();
    assert_eq!(
        extracted
            .records
            .iter()
            .filter(|r| r.schema == "v8.sample")
            .count(),
        2
    );
    let sample = extracted
        .records
        .iter()
        .find(|r| r.id == "sample:1")
        .unwrap();
    assert_eq!(sample.function.as_deref(), Some("resize"));
    assert_eq!(sample.time.as_ref().unwrap().start, 1300);
    let frame = extracted.records.iter().find(|r| r.id == "node:2").unwrap();
    assert_eq!(frame.payload["futureField"], true);
    assert!(extracted.records.iter().any(|r| r.schema == "v8.edge"));
    assert!(
        extracted
            .records
            .iter()
            .any(|r| r.schema == "v8.position-tick")
    );
    assert!(!extracted.warnings.is_empty());
}

#[test]
fn v8_rejects_missing_sample_nodes_and_cycles_and_mismatched_delta_counts() {
    let base = json!({"startTime":0,"endTime":1000,"nodes":[{"id":1,"callFrame":{"functionName":"f","scriptId":"1","url":"f.js","lineNumber":0,"columnNumber":0}}],"samples":[2],"timeDeltas":[100]});
    assert!(v8::extract(&serde_json::to_vec(&base).unwrap(), "p", Limits::default()).is_err());
    let mut cycle = base.clone();
    cycle["samples"] = json!([1]);
    cycle["nodes"][0]["children"] = json!([1]);
    assert!(v8::extract(&serde_json::to_vec(&cycle).unwrap(), "p", Limits::default()).is_err());
    let mut mismatch = base;
    mismatch["timeDeltas"] = json!([]);
    assert!(
        v8::extract(
            &serde_json::to_vec(&mismatch).unwrap(),
            "p",
            Limits::default()
        )
        .is_err()
    );
}

#[test]
fn python_keeps_call_edges_and_recursion_separate_from_elapsed_cost() {
    let input = json!({"runtime":"cpython","version":"3.14.6","functions":[
      {"id":0,"name":"parent","file":"fixture.py","line":2,"calls":3,"recursive_calls":1,"total_seconds":0.4,"self_seconds":0.1,"callees":[{"callee":1,"calls":3,"recursive_calls":0,"total_seconds":0.3,"self_seconds":0.2}]},
      {"id":1,"name":"resize","file":"fixture.py","line":9,"calls":3,"recursive_calls":0,"total_seconds":0.3,"self_seconds":0.2,"callees":[]}
    ]});
    let extraction =
        python::extract(&serde_json::to_vec(&input).unwrap(), "p", Limits::default()).unwrap();
    let function = extraction
        .records
        .iter()
        .find(|r| r.id == "function:0")
        .unwrap();
    assert_eq!(function.payload["recursive_calls"], 1);
    assert!(function.time.is_none());
    let edge = extraction
        .records
        .iter()
        .find(|r| r.schema == "python.cprofile-edge")
        .unwrap();
    assert_eq!(edge.payload["callee"], 1);
    assert_eq!(edge.payload["calls"], 3);
}

#[test]
fn extractor_enforces_input_and_record_budgets() {
    let limits = Limits {
        max_input_bytes: 2,
        ..Limits::default()
    };
    assert!(v8::extract(b"{} ", "p", limits).is_err());
    let input = json!({"runtime":"cpython","version":"3.14.6","functions":[]});
    let limits = Limits {
        max_records: 0,
        ..Limits::default()
    };
    assert!(python::extract(&serde_json::to_vec(&input).unwrap(), "p", limits).is_err());
}

#[test]
fn bun_fractional_profile_clock_stays_exact_in_payload_and_bounded_in_projection() {
    let input = json!({"startTime":1000.5,"endTime":1200.75,"nodes":[{"id":1,"callFrame":{"functionName":"resize","scriptId":"7","url":"fixture.js","lineNumber":0,"columnNumber":0}}],"samples":[1],"timeDeltas":[100]});
    let extraction = v8::extract(
        &serde_json::to_vec(&input).unwrap(),
        "bun",
        Limits::default(),
    )
    .unwrap();
    let header = extraction
        .records
        .iter()
        .find(|r| r.schema == "v8.profile")
        .unwrap();
    assert_eq!(header.payload["startTime"], 1000.5);
    assert_eq!(header.payload["endTime"], 1200.75);
    let sample = extraction
        .records
        .iter()
        .find(|r| r.schema == "v8.sample")
        .unwrap();
    assert_eq!(sample.time.as_ref().unwrap().start, 1100);
    assert_eq!(sample.time.as_ref().unwrap().uncertainty, 1);
}

#[test]
fn repeated_long_frame_projections_count_toward_the_entire_record_byte_budget() {
    let name = "n".repeat(12_000);
    let url = format!("file:///{}.js", "p".repeat(12_000));
    let input = json!({"startTime":0,"endTime":1000,"nodes":[{"id":1,"callFrame":{"functionName":name,"scriptId":"1","url":url,"lineNumber":0,"columnNumber":0}}],"samples":[1,1,1,1,1,1,1,1],"timeDeltas":[10,10,10,10,10,10,10,10]});
    let bytes = serde_json::to_vec(&input).unwrap();
    let limits = Limits {
        max_output_bytes: 100_000,
        ..Limits::default()
    };
    let error = v8::extract(&bytes, "long-frame", limits).unwrap_err();
    assert!(error.to_string().contains("byte budget"));
    let successful = v8::extract(
        &bytes,
        "long-frame",
        Limits {
            max_output_bytes: 400_000,
            ..Limits::default()
        },
    )
    .unwrap();
    let serialized = successful
        .records
        .iter()
        .map(|r| serde_json::to_vec(r).unwrap().len())
        .sum::<usize>();
    assert!(serialized > 100_000);
    assert!(serialized <= 400_000);
}

#[test]
fn preserved_orphan_callee_diagnostics_do_not_claim_dropped_capture_data() {
    let input = json!({"runtime":"pypy","version":"3.12.15","functions":[{"id":0,"name":"load","file":"fixture.py","line":1,"calls":1,"recursive_calls":0,"total_seconds":0.1,"self_seconds":0.1,"callees":[{"callee":null,"callee_code":["builtin","<built-in function openssl_sha256>"],"callee_resolution":"missing_aggregate","calls":0,"recursive_calls":0,"total_seconds":0.0,"self_seconds":0.0}]}]});
    let extraction = python::extract(
        &serde_json::to_vec(&input).unwrap(),
        "pypy",
        Limits::default(),
    )
    .unwrap();
    assert!(extraction.capture_complete);
    assert_eq!(
        extraction.metadata["callee_resolution"]["unresolved_edges"],
        1
    );
    assert_eq!(
        extraction.metadata["callee_resolution"]["reported_calls_on_unresolved_edges"],
        0
    );
    assert_eq!(extraction.warnings.len(), 1);
    assert!(
        extraction
            .records
            .iter()
            .any(|r| r.schema == "python.cprofile-edge"
                && r.payload["callee_code"][1] == "<built-in function openssl_sha256>")
    );
}

#[test]
fn missing_v8_stream_is_incomplete_but_unprojected_native_clock_fields_are_retained() {
    let input = json!({"startTime":0,"endTime":10,"nodes":[{"id":1,"callFrame":{"functionName":"f","scriptId":"1","url":"f.js","lineNumber":0,"columnNumber":0}}]});
    let extraction =
        v8::extract(&serde_json::to_vec(&input).unwrap(), "p", Limits::default()).unwrap();
    assert!(!extraction.capture_complete);
}

#[test]
fn source_reported_ruby_and_jfr_buffer_loss_is_explicit() {
    let ruby = json!({"runtime":"ruby","version":"4.0.9","events":[],"lost_events":3});
    let extracted = hunin_runtimes::ruby::extract(
        &serde_json::to_vec(&ruby).unwrap(),
        "ruby",
        Limits::default(),
    )
    .unwrap();
    assert!(!extracted.capture_complete);
    assert_eq!(extracted.records[0].payload["lost_events"], 3);
    let jfr = json!({"runtime":"hotspot","version":"25.0.5+8","samples":[],"period_ns":10000000,"lost_samples":2});
    let extracted =
        hunin_runtimes::jfr::extract(&serde_json::to_vec(&jfr).unwrap(), "jvm", Limits::default())
            .unwrap();
    assert!(!extracted.capture_complete);
    assert_eq!(extracted.records[0].payload["lost_samples"], 2);
}

#[test]
fn jfr_stack_truncation_and_native_loss_are_incomplete_without_erasing_available_samples() {
    let profile = json!({"runtime":"hotspot","version":"25.0.5+8","samples":[{"timestamp_ns":1,"state":"STATE_RUNNABLE","truncated":true,"frames":[]}],"period_ns":10000000,"lost_samples":0,"unsupported_event_types":{"jdk.DataLoss":1}});
    let extracted = hunin_runtimes::jfr::extract(
        &serde_json::to_vec(&profile).unwrap(),
        "jvm",
        Limits::default(),
    )
    .unwrap();
    assert!(!extracted.capture_complete);
    assert_eq!(
        extracted
            .records
            .iter()
            .filter(|r| r.schema == "jfr.sample")
            .count(),
        1
    );
    assert_eq!(
        extracted.records.last().unwrap().payload["unsupported_event_types"]["jdk.DataLoss"],
        1
    );
    assert_eq!(extracted.warnings.len(), 2);
}

#[test]
fn an_individual_retained_record_cannot_exceed_the_core_record_boundary() {
    let input = json!({"startTime":0,"endTime":10,"nodes":[{"id":1,"callFrame":{"functionName":"f","scriptId":"1","url":"f.js","lineNumber":0,"columnNumber":0},"future":vec![0;600_000]}],"samples":[1],"timeDeltas":[1]});
    let error =
        v8::extract(&serde_json::to_vec(&input).unwrap(), "p", Limits::default()).unwrap_err();
    assert!(error.to_string().contains("1 MiB"));
}

use hunin_runtimes::{Limits, browser::safari};
use serde_json::{Value, json};
fn profile() -> Value {
    json!({"format":"hunin.safari-instrument-v1","time_origin_ms":1000,"start_ms":1,"end_ms":4,"scripts":[{"id":"script-1","transform_id":"b".repeat(64),"source_name":"fixture.js","original_sha256":"a".repeat(64),"instrumented_sha256":"c".repeat(64)}],"functions":[{"id":"script-1:0","script":"script-1","ordinal":0,"name":"resize","kind":"FunctionDeclaration","async":false,"generator":false,"line":0,"column":0,"end_line":1,"end_column":0}],"events":[{"call_id":1,"function_id":"script-1:0","kind":"entry","time_ms":2,"body_throw":null},{"call_id":1,"function_id":"script-1:0","kind":"exit","time_ms":3,"body_throw":false}],"lost_events":0,"hook_errors":0,"first_loss_ms":null,"open_calls":[],"semantics":{"elapsed":"body wall interval"}})
}
#[test]
fn safari_export_preserves_native_ms_and_separates_declarations_from_execution() {
    let extracted = safari::extract(
        &serde_json::to_vec(&profile()).unwrap(),
        "page",
        Limits::default(),
    )
    .unwrap();
    assert!(extracted.capture_complete);
    let call = extracted
        .records
        .iter()
        .find(|r| r.schema == "safari.call")
        .unwrap();
    assert_eq!(call.payload["time_ms"], 2);
    assert_eq!(call.time.as_ref().unwrap().start, 2000);
    assert_eq!(call.function.as_deref(), Some("resize"));
    assert_eq!(call.location.as_deref(), Some("fixture.js:1:1"));
    assert!(
        extracted
            .records
            .iter()
            .any(|r| r.schema == "safari.function")
    );
}
#[test]
fn safari_rejects_clock_declaration_outcome_and_binding_corruption_before_ingest() {
    let original = profile();
    let mut cases = vec![];
    for field in ["time_origin_ms", "start_ms", "end_ms"] {
        let mut value = original.clone();
        value[field] = json!(-1);
        cases.push(value);
    }
    let mut v = original.clone();
    v["end_ms"] = json!(0);
    cases.push(v);
    let mut v = original.clone();
    v["events"][0]["time_ms"] = json!(0);
    cases.push(v);
    let mut v = original.clone();
    v["events"][1]["time_ms"] = json!(5);
    cases.push(v);
    let mut v = original.clone();
    v["events"][0]["body_throw"] = json!(false);
    cases.push(v);
    let mut v = original.clone();
    v["events"][1]["body_throw"] = Value::Null;
    cases.push(v);
    for field in ["async", "generator"] {
        let mut v = original.clone();
        v["functions"][0][field] = json!("true");
        cases.push(v);
    }
    let mut v = original.clone();
    v["functions"][0]["kind"] = json!("unknown");
    cases.push(v);
    let mut v = original.clone();
    v["scripts"][0]["instrumented_sha256"] = Value::Null;
    cases.push(v);
    let mut v = original.clone();
    v["functions"][0]["line"] = json!(u64::MAX);
    v["functions"][0]["end_line"] = json!(u64::MAX);
    cases.push(v);
    for open in [json!([0]), json!([1, 1]), json!([2])] {
        let mut v = original.clone();
        v["open_calls"] = open;
        cases.push(v);
    }
    for loss in [json!(-1), json!(5), json!(2)] {
        let mut v = original.clone();
        v["first_loss_ms"] = loss;
        cases.push(v);
    }
    for value in cases {
        assert!(
            safari::extract(
                &serde_json::to_vec(&value).unwrap(),
                "page",
                Limits::default()
            )
            .is_err(),
            "{value}"
        );
    }
}
#[test]
fn safari_loss_keeps_available_prefix_and_does_not_invent_missing_completion() {
    let mut input = profile();
    input["events"].as_array_mut().unwrap().pop();
    input["lost_events"] = json!(1);
    input["first_loss_ms"] = json!(3);
    let extracted = safari::extract(
        &serde_json::to_vec(&input).unwrap(),
        "page",
        Limits::default(),
    )
    .unwrap();
    assert!(!extracted.capture_complete);
    assert_eq!(
        extracted
            .records
            .iter()
            .filter(|r| r.schema == "safari.call")
            .count(),
        1
    );
    assert_eq!(extracted.records.last().unwrap().payload["lost_events"], 1);
}

#[cfg(unix)]
#[test]
fn safari_compiler_rejects_a_symlink_as_the_owned_output_area() {
    use std::{os::unix::fs::symlink, path::Path};
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    let alias = root.path().join("alias");
    std::fs::create_dir(&real).unwrap();
    symlink(&real, &alias).unwrap();
    let error = safari::instrument(
        Path::new("/absent/node"),
        Path::new("/absent/babel"),
        Path::new("/absent/source"),
        &alias.join("out.js"),
        &alias.join("binding.json"),
        &alias,
    )
    .unwrap_err();
    assert!(error.to_string().contains("owned directory"));
    assert_eq!(std::fs::read_dir(real).unwrap().count(), 0);
}

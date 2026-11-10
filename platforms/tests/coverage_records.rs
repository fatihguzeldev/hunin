use hunin_core::{
    project::Project,
    query::{Query, QueryLimits},
};
use hunin_platforms::native::{Language, coverage::parse_export};
use serde_json::{Value, json};
const FIXTURE: &str = include_str!("fixtures/llvm_coverage.json");
#[test]
fn coverage_retains_function_regions_file_segments_and_queries_after_reopen() {
    let snapshot = parse_export(
        FIXTURE,
        &"a".repeat(64),
        Language::Cpp,
        "after",
        json!({"origin":"parser fixture"}),
    )
    .unwrap();
    let records = snapshot.records("native");
    let function = records
        .iter()
        .find(|r| {
            r.function.as_deref() == Some("_Z14resize_previewPKhPhm")
                && r.schema.ends_with(".function")
        })
        .unwrap();
    assert_eq!(function.payload["count"], 7);
    assert!(records.iter().any(|r| r.schema.ends_with(".region")
        && r.location.as_deref() == Some("/build/coverage.cpp:5")
        && r.payload["count"] == 7));
    assert!(records.iter().any(|r| r.schema.ends_with(".segment")));
    assert!(records.iter().any(|r| r.schema.ends_with(".file_branch")));
    assert!(records.iter().any(|r| r.schema.ends_with(".totals")));
    let temporary = tempfile::tempdir().unwrap();
    let project = Project::create(temporary.path(), "coverage").unwrap();
    let investigation = project
        .create_investigation("i", "coverage", json!({}))
        .unwrap();
    let run = project.create_run(&investigation, "r", json!({})).unwrap();
    let target = project.register_target(&run, "t", json!({}), true).unwrap();
    let source = project
        .register_source(&run, snapshot.definition("counters", target))
        .unwrap();
    source.append(&records).unwrap();
    drop(source);
    drop(project);
    let project = Project::open(temporary.path(), "coverage").unwrap();
    let mut query = Query::new(run);
    query.function = Some("_Z14resize_previewPKhPhm".into());
    let page = project.query(&query, QueryLimits::default(), None).unwrap();
    assert_eq!(page.records.len(), 6);
    assert_eq!(
        project
            .record(
                &project
                    .source(&project.reference("r"), "counters")
                    .unwrap()
                    .record_ref(&function.id),
                None
            )
            .unwrap(),
        *function
    );
}
#[test]
fn unsupported_or_corrupt_coverage_is_never_a_successful_summary_import() {
    let mut root: Value = serde_json::from_str(FIXTURE).unwrap();
    root["version"] = "99".into();
    assert!(
        parse_export(
            &root.to_string(),
            &"a".repeat(64),
            Language::C,
            "after",
            json!({})
        )
        .is_err()
    );
    root["version"] = "3.0.1".into();
    root["data"][0]["functions"][0]["count"] = "seven".into();
    assert!(
        parse_export(
            &root.to_string(),
            &"a".repeat(64),
            Language::C,
            "after",
            json!({})
        )
        .is_err()
    );
    root["data"][0]["functions"] = json!([]);
    assert!(
        parse_export(
            &root.to_string(),
            &"a".repeat(64),
            Language::C,
            "after",
            json!({})
        )
        .is_err()
    );
}

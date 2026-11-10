use hunin_core::{
    project::Project,
    query::{Query, QueryLimits},
};
use hunin_platforms::native::{Language, macos::parse_sample};
use serde_json::json;
#[test]
fn native_records_are_queryable_and_survive_original_deletion_and_reopen() {
    let temporary = tempfile::tempdir().unwrap();
    let project = Project::create(temporary.path(), "native").unwrap();
    let investigation = project
        .create_investigation("i", "native", json!({}))
        .unwrap();
    let run = project.create_run(&investigation, "r", json!({})).unwrap();
    let target = project
        .register_target(&run, "t", json!({"pid":2}), true)
        .unwrap();
    let text = "Process: demo [2]\nCode Type: ARM-64\nReport Version: 7\n\nCall graph:\n    1 Thread_7\n    + 1 render (in demo) [0x100000010] preview.cpp:17\n\nBinary Images:\n       0x100000000 -        0x10000ffff +demo (1) <00112233-4455-6677-8899-AABBCCDDEEFF> /tmp/demo\n";
    let raw = temporary.path().join("sample.txt");
    std::fs::write(&raw, text).unwrap();
    let mut profile = parse_sample(&std::fs::read_to_string(&raw).unwrap(), Language::Cpp).unwrap();
    profile.aggregates[0]
        .frame
        .inline_sites
        .push(hunin_platforms::native::InlineSite {
            function: Some("outer_function".into()),
            source: Some(hunin_platforms::native::SourceLocation {
                file: "outer.rs".into(),
                line: 7,
                column: None,
            }),
        });
    assert!(
        profile
            .records("t")
            .unwrap()
            .iter()
            .any(
                |record| record.function.as_deref() == Some("outer_function")
                    && record.location.as_deref() == Some("outer.rs:7")
            )
    );
    assert!(
        profile
            .records("t")
            .unwrap()
            .iter()
            .any(|record| record.schema == "native.capture" && record.payload["issues"].is_array())
    );
    let source = project
        .register_source(&run, profile.definition("code", target))
        .unwrap();
    source.append(&profile.records("t").unwrap()).unwrap();
    std::fs::remove_file(raw).unwrap();
    drop(source);
    drop(project);
    let project = Project::open(temporary.path(), "native").unwrap();
    let mut query = Query::new(run);
    query.function = Some("render".into());
    let page = project.query(&query, QueryLimits::default(), None).unwrap();
    assert_eq!(page.records.len(), 1);
    let record = page.records[0].record.as_ref().unwrap();
    assert_eq!(record.payload["count"], 1);
    assert_eq!(record.payload["frame"]["address"], json!(0x100000010_u64));
    assert_eq!(record.location.as_deref(), Some("preview.cpp:17"));
}

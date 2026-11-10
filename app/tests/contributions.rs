use hunin_app::{BeginCapture, CapturePlan};
use hunin_core::{
    channel::ProducerListener,
    control::RequestId,
    project::Project,
    query::{Direction, Origin, Query, QueryLimits, Relationship},
    records::{Clock, FieldSchema, FieldType, RecordSchema, SourceDefinition},
};
use hunin_runtimes::RuntimeKind;
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

#[test]
#[ignore = "requires selected HUNIN_PYTHON runtime"]
fn external_process_contributes_a_live_measurement_with_bidirectional_evidence_links() {
    let python =
        PathBuf::from(std::env::var("HUNIN_PYTHON").expect("set selected absolute python"));
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workload = root
        .parent()
        .unwrap()
        .join("runtimes/tests/fixtures/preview.py");
    let expected_size = std::fs::metadata(&workload).unwrap().len();
    let data = tempfile::tempdir().unwrap();
    let project = Project::create(data.path(), "p").unwrap();
    let i = project
        .create_investigation("i", "external measurement", json!({}))
        .unwrap();
    let run = project
        .create_run(&i, "r", json!({"input":"fixed pixel vector"}))
        .unwrap();
    let request = RequestId {
        project: "p".into(),
        caller: "agent".into(),
        id: "capture".into(),
    };
    let BeginCapture::Started(mut capture) = hunin_app::begin(
        project.clone(),
        CapturePlan {
            request,
            run: run.clone(),
            runtime: RuntimeKind::CPython,
            executable: python.clone(),
            workload: workload.clone(),
            conditions: json!({}),
            environment: BTreeMap::new(),
            read_paths: vec![],
            write_paths: vec![],
            xdebug_extension: None,
        },
    )
    .unwrap() else {
        panic!("new capture")
    };
    let source=project.register_source(&run,SourceDefinition{id:"file-stat".into(),target:None,schemas:vec![RecordSchema{name:"external.file-stat".into(),version:1,description:"independent producer measures the prepared module's file size".into(),fields:BTreeMap::from([("value".into(),FieldSchema{kind:FieldType::Unsigned,required:true,unit:Some("bytes".into()),meaning:"file length from operating system stat".into()}),("path".into(),FieldSchema{kind:FieldType::String,required:true,unit:None,meaning:"explicitly selected module path".into()})])}],clocks:vec![Clock{id:"driver-monotonic".into(),ticks_per_second:1_000_000_000,description:"external driver's monotonic clock; no implicit mapping to profiler clocks".into()}],interpretation:"filesystem-stat-v1".into(),metadata:json!({"origin":"external process","operation_binding":"contributed module selection, not cpu attribution"})}).unwrap();
    let listener = ProducerListener::bind(source.clone()).unwrap();
    let address = listener.address().unwrap();
    let credential = listener.credential().to_owned();
    let server = std::thread::spawn(move || {
        let mut session = listener.accept(Duration::from_secs(3)).unwrap();
        for _ in 0..3 {
            session.next(Duration::from_secs(3)).unwrap();
        }
    });
    let mut driver = std::process::Command::new(python)
        .arg(root.join("tests/fixtures/producer.py"))
        .args([
            address.ip().to_string(),
            address.port().to_string(),
            "p".into(),
            "file-stat".into(),
            workload.to_string_lossy().into_owned(),
        ])
        .env("HUNIN_PRODUCER_CREDENTIAL", credential)
        .spawn()
        .unwrap();
    capture.release().unwrap();
    let report = capture.finish(Duration::from_secs(10)).unwrap();
    assert!(driver.wait().unwrap().success());
    server.join().unwrap();
    let mut measurement = Query::new(run.clone());
    measurement.operation = Some("preview-operation".into());
    let found = project
        .query(&measurement, QueryLimits::default(), None)
        .unwrap();
    assert_eq!(found.records.len(), 1);
    let metric = found.records[0].reference.clone();
    assert_eq!(
        project.record(&metric, None).unwrap().payload["value"],
        expected_size
    );
    let mut functions = Query::new(run.clone());
    functions.source = Some(report.code_sources[0].clone());
    functions.function = Some("resize".into());
    let evidence = project
        .query(&functions, QueryLimits::default(), None)
        .unwrap()
        .records[0]
        .reference
        .clone();
    project.add_relationship(&run,Relationship{id:"selected-code".into(),from:metric.clone(),to:evidence.clone(),kind:"module-input-for".into(),origin:Origin::Contributed,corrects:None,metadata:json!({"meaning":"agent connected module size to selected code evidence; no resource ownership claim"})}).unwrap();
    assert_eq!(
        project
            .relationships(
                &metric,
                Direction::Forward,
                None,
                QueryLimits::default(),
                None
            )
            .unwrap()
            .relationships
            .len(),
        1
    );
    assert_eq!(
        project
            .relationships(
                &evidence,
                Direction::Reverse,
                None,
                QueryLimits::default(),
                None
            )
            .unwrap()
            .relationships
            .len(),
        1
    );
    let mut resource_query = measurement.clone();
    resource_query.source = Some(report.resource_sources[0].clone());
    assert!(
        project
            .query(&resource_query, QueryLimits::default(), None)
            .unwrap()
            .records
            .is_empty(),
        "shared run and overlap cannot assign process resources to the operation"
    );
    drop(source);
    drop(project);
    let reopened = Project::open(data.path(), "p").unwrap();
    assert_eq!(
        reopened.record(&metric, None).unwrap().payload["value"],
        expected_size
    );
    assert_eq!(
        reopened
            .relationships(
                &evidence,
                Direction::Reverse,
                None,
                QueryLimits::default(),
                None
            )
            .unwrap()
            .relationships
            .len(),
        1
    );
}

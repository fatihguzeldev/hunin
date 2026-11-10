use hunin_core::{
    control::{Action, Controller, RequestId},
    execution::PreparedProcess,
    project::Project,
    query::{Context, Direction, Origin, Query, QueryLimits, Relationship},
    records::{FieldSchema, FieldType, Record, RecordSchema, SourceDefinition},
};
use hunin_runtimes::{Limits, RuntimeKind, prepare, v8};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

fn profile_bytes(area: &Path, path: &Path) -> Vec<u8> {
    assert!(path.is_absolute());
    assert!(fs::symlink_metadata(path).unwrap().file_type().is_file());
    let parent = fs::canonicalize(path.parent().unwrap()).unwrap();
    assert_eq!(parent, fs::canonicalize(area).unwrap());
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(path).unwrap();
    assert!(file.metadata().unwrap().is_file());
    let mut bytes = vec![];
    file.take(Limits::default().max_input_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes.len() <= Limits::default().max_input_bytes);
    bytes
}

#[test]
#[ignore = "requires selected HUNIN_NODE"]
fn worker_handoffs_keep_transport_origins_separate_code_sources_and_reopenable_edges() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "handoff").unwrap();
    let investigation = project
        .create_investigation("i", "preview worker handoff", json!({}))
        .unwrap();
    let run=project.create_run(&investigation,"r",json!({"work_count":10000,"scope":"single preview job plus concurrent unrelated worker"})).unwrap();
    let area = project.directory().join("tmp/prepared");
    fs::create_dir(&area).unwrap();
    let input = area.join("input");
    let output = area.join("output");
    fs::create_dir(&input).unwrap();
    fs::create_dir(&output).unwrap();
    let mut image = b"P6\n64 48\n255\n".to_vec();
    image.extend((0..64 * 48 * 3).map(|i| (i % 251) as u8));
    fs::write(input.join("one.ppm"), image).unwrap();
    let exe = PathBuf::from(std::env::var("HUNIN_NODE").unwrap());
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/asset-handoff.cjs");
    let mut prepared = prepare(RuntimeKind::Node, &exe, &fixture, &area).unwrap();
    prepared
        .configure(
            &BTreeMap::from([
                ("PREVIEW_INPUT".into(), input.to_string_lossy().into_owned()),
                (
                    "PREVIEW_OUTPUT".into(),
                    output.to_string_lossy().into_owned(),
                ),
                ("PREVIEW_REPETITIONS".into(), "10000".into()),
            ]),
            &[input],
            std::slice::from_ref(&output),
        )
        .unwrap();
    let controller = Controller::new(project.clone());
    let launch = RequestId {
        project: "handoff".into(),
        caller: "fixture-harness".into(),
        id: "launch".into(),
    };
    controller
        .accept(
            &launch,
            &run,
            Action::Launch,
            json!({"program":prepared.launch.program,"fixture":"node worker handoff"}),
        )
        .unwrap();
    let effect = controller.dispatch(&launch).unwrap();
    let mut process = PreparedProcess::spawn(&prepared.launch).unwrap();
    let target = project
        .register_target(&run, "node-process", json!({"pid":process.id()}), true)
        .unwrap();
    let bootstrap = project
        .register_source(&run, prepared.definition("bootstrap-code", &target))
        .unwrap();
    process.await_ready(Duration::from_secs(10)).unwrap();
    bootstrap
        .ready(json!({"scope":"bootstrap isolate; worker sources acknowledged separately"}))
        .unwrap();
    process.release().unwrap();
    controller
        .complete(
            &launch,
            effect.generation,
            true,
            json!({"pid":process.id(),"required_source_ready":true}),
        )
        .unwrap();
    let outcome = process.wait(Duration::from_secs(30)).unwrap();
    assert!(outcome.success, "{outcome:?}");
    let main = prepared
        .extract("node-bootstrap", Limits::default())
        .unwrap();
    for batch in main.records.chunks(256) {
        bootstrap.append(batch).unwrap();
    }
    bootstrap.close(json!({"complete":true})).unwrap();
    let report = main
        .records
        .iter()
        .find(|r| r.schema == "prepared.report")
        .unwrap();
    assert_eq!(report.payload["outcome"], "completed");
    let result = &report.payload["output"];
    let events = result["events"].as_array().unwrap();
    let workers = result["worker_sources"].as_array().unwrap();
    assert_eq!(workers.len(), 2);
    assert_ne!(workers[0]["thread_id"], workers[1]["thread_id"]);
    let fields = BTreeMap::from([
        (
            "id".into(),
            FieldSchema {
                kind: FieldType::String,
                required: true,
                unit: None,
                meaning: "exact message id propagated by this fixture".into(),
            },
        ),
        (
            "kind".into(),
            FieldSchema {
                kind: FieldType::String,
                required: true,
                unit: None,
                meaning: "instrumented enqueue/dequeue/start/finish".into(),
            },
        ),
        (
            "thread_id".into(),
            FieldSchema {
                kind: FieldType::Unsigned,
                required: true,
                unit: None,
                meaning: "Node worker id, not an OS thread id".into(),
            },
        ),
        (
            "monotonic_ns".into(),
            FieldSchema {
                kind: FieldType::String,
                required: true,
                unit: Some("ns".into()),
                meaning: "raw helper hrtime; no projection into the CPU-profile clock".into(),
            },
        ),
    ]);
    let transport=project.register_source(&run,SourceDefinition{id:"transport".into(),target:Some(target.clone()),schemas:vec![RecordSchema{name:"fixture.handoff".into(),version:1,fields,description:"explicit fixture-owned message receipts".into()}],clocks:vec![],interpretation:"fixture-node-handoff-v1".into(),metadata:json!({"origin":"instrumented","ingest":"deferred; event clocks and readiness receipts retain their actual meaning"})}).unwrap();
    transport
        .ready(json!({"basis":"worker profiler acknowledged before each message was sent"}))
        .unwrap();
    let mut references = BTreeMap::new();
    for (index, event) in events.iter().enumerate() {
        let operation = event["id"].as_str().unwrap();
        let kind = event["kind"].as_str().unwrap();
        let id = format!("message:{index}");
        let row = Record {
            id: id.clone(),
            schema: "fixture.handoff".into(),
            schema_version: 1,
            subject: format!("node-worker:{}", event["thread_id"]),
            time: None,
            function: None,
            location: None,
            operation: Some(operation.into()),
            payload: event.clone(),
        };
        transport.append(&[row]).unwrap();
        references.insert(
            (operation.to_string(), kind.to_string()),
            transport.record_ref(id),
        );
    }
    transport.close(json!({"complete":true})).unwrap();
    let mut checks = vec![];
    for worker in workers {
        let operation = worker["operation"].as_str().unwrap();
        let thread = worker["thread_id"].as_u64().unwrap();
        let role = worker["role"].as_str().unwrap();
        let enqueue = events
            .iter()
            .find(|e| e["id"] == operation && e["kind"] == "enqueue")
            .unwrap();
        let dequeue = events
            .iter()
            .find(|e| e["id"] == operation && e["kind"] == "dequeue")
            .unwrap();
        assert_eq!(enqueue["destination_thread_id"], thread);
        assert_eq!(dequeue["thread_id"], thread);
        assert!(
            worker["source_ready_ns"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                <= dequeue["monotonic_ns"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
        );
        let path = PathBuf::from(worker["profile_path"].as_str().unwrap());
        let bytes = profile_bytes(&area, &path);
        let source_id = format!("worker-{thread}-code");
        let extraction =
            v8::extract(&bytes, &format!("node-worker:{thread}"), Limits::default()).unwrap();
        let expected = if role == "preview" {
            "resize"
        } else {
            "unrelatedWork"
        };
        let excluded = if role == "preview" {
            "unrelatedWork"
        } else {
            "resize"
        };
        let sample = extraction
            .records
            .iter()
            .find(|r| r.schema == "v8.sample" && r.function.as_deref() == Some(expected))
            .expect("known worker function sample");
        assert!(
            !extraction
                .records
                .iter()
                .any(|r| r.schema == "v8.sample" && r.function.as_deref() == Some(excluded))
        );
        assert!(
            sample.operation.is_none(),
            "profile itself supplies no operation ownership"
        );
        let source=project.register_source(&run,SourceDefinition{id:source_id.clone(),target:Some(target.clone()),schemas:extraction.schemas.clone(),clocks:extraction.clocks.clone(),interpretation:"v8-profile-v1".into(),metadata:json!({"thread_id":thread,"thread_kind":"node-worker","role":role,"scope":"one worker isolate, including its idle/setup tail","origin":"captured"})}).unwrap();
        source.ready(json!({"actual_ready_ns":worker["source_ready_ns"],"deferred_ingest":true,"message_gate_verified":true})).unwrap();
        for batch in extraction.records.chunks(256) {
            source.append(batch).unwrap();
        }
        source.close(json!({"complete":true})).unwrap();
        let from = references[&(operation.to_string(), "enqueue".to_string())].clone();
        let to = references[&(operation.to_string(), "dequeue".to_string())].clone();
        project.add_relationship(&run,Relationship{id:format!("handoff:{thread}"),from:from.clone(),to:to.clone(),kind:"causes".into(),origin:Origin::Instrumented,corrects:None,metadata:json!({"message_id":operation,"basis":"identical id carried by actual postMessage and receipt"})}).unwrap();
        let sampled = source.record_ref(sample.id.clone());
        project.add_relationship(&run,Relationship{id:format!("job-code:{thread}"),from:to.clone(),to:sampled.clone(),kind:"observed_in".into(),origin:Origin::Contributed,corrects:None,metadata:json!({"message_id":operation,"basis":"fixture has exactly one job in this worker; known function is called by that job","does_not_claim":"all worker cpu belongs to the job"})}).unwrap();
        project.add_context(&run,Context{id:format!("job:{thread}"),subject:operation.into(),origin:Origin::Contributed,corrects:None,value:json!({"role":role,"meaning":"fixture harness supplies domain context; core does not infer it from timestamps"})}).unwrap();
        checks.push((
            source_id,
            operation.to_string(),
            expected.to_string(),
            excluded.to_string(),
            from,
            to,
            sampled,
        ));
    }
    let manifest: Value =
        serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest, result["manifest"]);
    let mut expected = b"P6\n32 32\n255\n".to_vec();
    for y in 0..32 {
        for x in 0..32 {
            for c in 0..3 {
                expected.push(((((y * 48 / 32) * 64 + x * 64 / 32) * 3 + c) % 251) as u8);
            }
        }
    }
    assert_eq!(fs::read(output.join("one.preview.ppm")).unwrap(), expected);
    let checksum = (0..32u64)
        .map(|pass| (pass * 997 * 17 + pass) as u32)
        .fold(0u32, |sum, value| sum.wrapping_add(value));
    assert_eq!(result["unrelated_checksum"], u64::from(checksum));
    fs::remove_dir_all(&area).unwrap();
    drop(bootstrap);
    drop(transport);
    let finish = RequestId {
        project: "handoff".into(),
        caller: "fixture-harness".into(),
        id: "finish".into(),
    };
    controller
        .accept(&finish, &run, Action::Finalize, json!({}))
        .unwrap();
    let effect = controller.dispatch(&finish).unwrap();
    controller
        .complete(
            &finish,
            effect.generation,
            true,
            json!({"owned_process_exited":true,"sources_closed":true,"raw_removed":true}),
        )
        .unwrap();
    drop(controller);
    drop(project);
    let reopened = Project::open(directory.path(), "handoff").unwrap();
    for (source, operation, expected, excluded, from, to, sampled) in checks {
        let mut query = Query::new(run.clone());
        query.source = Some(source);
        query.schema = Some("v8.sample".into());
        query.function = Some(expected);
        assert!(
            !reopened
                .query(&query, QueryLimits::default(), None)
                .unwrap()
                .records
                .is_empty()
        );
        query.function = Some(excluded);
        assert!(
            reopened
                .query(&query, QueryLimits::default(), None)
                .unwrap()
                .records
                .is_empty()
        );
        let forward = reopened
            .relationships(
                &from,
                Direction::Forward,
                None,
                QueryLimits::default(),
                None,
            )
            .unwrap();
        assert!(forward.relationships.iter().any(|e| {
            e.from_present
                && e.to_present
                && e.relationship
                    .as_ref()
                    .is_some_and(|r| r.to == to && r.origin == Origin::Instrumented)
        }));
        let reverse = reopened
            .relationships(&to, Direction::Reverse, None, QueryLimits::default(), None)
            .unwrap();
        assert!(
            reverse
                .relationships
                .iter()
                .any(|e| e.relationship.as_ref().is_some_and(|r| r.from == from))
        );
        let code = reopened
            .relationships(
                &sampled,
                Direction::Reverse,
                None,
                QueryLimits::default(),
                None,
            )
            .unwrap();
        assert_eq!(code.relationships.len(), 1);
        assert_eq!(
            code.relationships[0]
                .relationship
                .as_ref()
                .unwrap()
                .metadata["message_id"],
            operation
        );
        if operation == "unrelated-job-1" {
            assert!(code.relationships.iter().all(
                |e| e.relationship.as_ref().unwrap().metadata["message_id"] != "preview-job-1"
            ));
        }
    }
}

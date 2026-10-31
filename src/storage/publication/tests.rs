use super::*;
use crate::model::runs::{Run, RunStatus};
use serde_json::{Value, json};
use std::time::Duration;

#[test]
fn publication_waiting_for_the_writer_cannot_outlive_its_deadline() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let run = Run {
        id: "recording".into(),
        name: "deadline control".into(),
        status: RunStatus::Completed,
        command: vec!["fixture".into()],
        cwd: ".".into(),
        runtime: "none".into(),
        started_at_us: 1,
        finished_at_us: Some(2),
        exit_code: Some(0),
        diagnostics: vec![],
        source: json!({}),
        correctness: json!({"passed":true}),
    };
    store.save_run(&run).unwrap();
    let event = |id| json!({"schema_version":1,"kind":"event","id":id,"run_id":"recording","producer_id":"fixture","pid":1,"tid":"main","timestamp_us":1,"name":"control","basis":"observed"});
    store
        .accept("recording", "source", &[event("before")])
        .unwrap();
    store
        .save_document(
            "analysis-results",
            "attempt",
            &json!({"run_id":"recording","status":"running"}),
        )
        .unwrap();
    let mut blocker = store.connection().unwrap();
    let lock = blocker
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let publishing = store.clone();
    let (ready, waiting) = std::sync::mpsc::channel();
    let task = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_millis(25);
        ready.send(()).unwrap();
        publishing.publish_result(PublicationRequest {
            run_id: "recording",
            source_key: "source",
            input_identity: "expired-result",
            records: &[event("after")],
            metadata: Some(PublicationMetadata::Analysis {
                execution_id: "attempt",
                document: &json!({"run_id":"recording","status":"accepted"}),
            }),
            deadline: Some(deadline),
        })
    });
    waiting.recv().unwrap();
    std::thread::sleep(Duration::from_millis(75));
    lock.commit().unwrap();
    let error = task
        .join()
        .unwrap()
        .expect_err("expired publication was accepted after waiting for the writer");
    assert!(error.to_string().contains("deadline"), "{error:#}");
    assert_eq!(store.records("recording").unwrap(), vec![event("before")]);
    assert_eq!(
        store.document("analysis-results", "attempt").unwrap()["status"],
        "running"
    );
}

fn run() -> Run {
    Run {
        id: "run-1".into(),
        name: "control".into(),
        status: "completed".into(),
        command: vec!["control".into()],
        cwd: "/tmp".into(),
        runtime: "native".into(),
        started_at_us: 1,
        finished_at_us: Some(5),
        exit_code: Some(0),
        diagnostics: vec![],
        source: json!({"fingerprint":"source-1"}),
        correctness: json!({"passed":true}),
    }
}
fn event(id: &str) -> Value {
    json!({"schema_version":1,"kind":"event","id":id,"run_id":"run-1","producer_id":"process-lifetime-1","pid":123,"tid":"main","timestamp_us":2,"name":"control","basis":"observed"})
}

fn runtime_code(id: &str) -> Value {
    let mut value = event(id);
    value["name"] = json!("code.instance");
    value["attributes"] = json!({"code_instance":{"schema_version":1,"instance_kind":"runtime","runtime_instance_id":"process-generation-1","representation":"machine_code","range":{"space":"runtime_address","start":"0x1000","end_exclusive":"0x1100"},"validity":{"clock":"unix_epoch","alignment":"direct_epoch","start_us":1,"end_us":5,"start_censored":false,"end_censored":false,"boundary_basis":"fixture"}}});
    value
}

#[test]
fn frame_instances_are_typed_dependencies_and_superseding_code_invalidates_samples() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    store
        .accept_import("run-1", "code", "one", &[runtime_code("code-one")])
        .unwrap();
    let sample = json!({"schema_version":1,"kind":"sample","id":"sample","run_id":"run-1","producer_id":"fixture","pid":123,"tid":"main","timestamp_us":2,"basis":"sampled","weight":1,"weight_unit":"samples","stack":[{"function":"work","code_instance_id":"code-one","code_offset":"0x1001"}],"attributes":{"clock":"unix_epoch","clock_alignment":"direct_epoch"}});
    store.accept("run-1", "samples", &[sample]).unwrap();
    store
        .accept_import("run-1", "code", "two", &[runtime_code("code-two")])
        .unwrap();
    assert!(
        store
            .records("run-1")
            .unwrap()
            .iter()
            .all(|row| row["id"] != "sample")
    );
    let mut bad = runtime_code("bad");
    bad["attributes"]["code_instance"]["instance_kind"] = json!("static");
    assert!(store.accept("run-1", "bad", &[bad]).is_err());
}

#[test]
fn code_artifact_corruption_and_non_instance_endpoints_cannot_publish() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let artifact = store
        .add_artifact_bytes("run-1", "code.bin", "raw", b"code")
        .unwrap();
    let mut code = runtime_code("code");
    code["attributes"]["code_instance"]["artifact"] =
        json!({"artifact_id":artifact.id,"sha256":artifact.sha256});
    std::fs::write(store.artifact_path("run-1", &artifact.id).unwrap(), b"oops").unwrap();
    assert!(store.accept("run-1", "corrupt-code", &[code]).is_err());
    store
        .accept("run-1", "ordinary", &[event("ordinary")])
        .unwrap();
    let mapping = json!({"schema_version":1,"kind":"relation","id":"mapping","run_id":"run-1","producer_id":"fixture","pid":123,"tid":"main","timestamp_us":2,"basis":"derived","from":"ordinary","to":"ordinary","relation":"code_maps_to","evidence":["ordinary"],"attributes":{"code_mapping":{"schema_version":1,"from_range":{"space":"runtime_address","start":"0x1000","end_exclusive":"0x1001"},"to_range":{"space":"runtime_address","start":"0x1000","end_exclusive":"0x1001"},"cardinality":"exclusive","proof":{"artifact_refs":[],"method":"fixture","verification":"declared"}}}});
    assert!(
        store
            .accept("run-1", "invalid-mapping", &[mapping])
            .is_err()
    );
}

#[test]
fn code_proof_alias_metadata_cannot_expand_a_verified_blob() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let first = store
        .add_artifact_bytes("run-1", "first.bin", "raw", b"code")
        .unwrap();
    let alias = store
        .add_artifact_bytes("run-1", "alias.bin", "raw", b"code")
        .unwrap();
    store.connection().unwrap().execute(
        "UPDATE artifacts SET metadata=json_set(metadata,'$.size',100) WHERE run_id=?1 AND id=?2",
        params!["run-1", alias.id],
    ).unwrap();
    let mut records = vec![runtime_code("first"), runtime_code("alias")];
    for (record, artifact) in records.iter_mut().zip([first, alias]) {
        record["attributes"]["code_instance"]["artifact"] =
            json!({"artifact_id":artifact.id,"sha256":artifact.sha256});
    }
    let error =
        super::code::prepare_artifacts(&store, "run-1", &records, &mut || Ok(())).unwrap_err();
    assert!(error.to_string().contains("size"), "{error:#}");
}

#[test]
fn unrelated_opaque_attributes_do_not_acquire_code_reference_semantics() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let mut ordinary = event("opaque");
    ordinary["attributes"] = json!({"code_mapping":{"proof":{"semantic_evidence":["unresolved-opaque-value"]}},"code_mapping_unavailable":{"anchor":{"observation_id":"opaque-value"}}});
    assert!(store.accept("run-1", "opaque", &[ordinary]).unwrap());
}

#[test]
fn reanalysis_invalidates_dependents_and_keeps_independent_results() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    store.accept("run-1", "raw", &[event("raw")]).unwrap();
    let raw = store.selected_result_id("run-1", "raw").unwrap().unwrap();
    let publish = |source: &str, id: &str, inputs: Vec<String>| {
        let mut record = event(id);
        record["basis"] = json!("derived");
        let doc = json!({"run_id":"run-1","status":"accepted","input_result_sets":inputs});
        store.publish_result(PublicationRequest {
            run_id: "run-1",
            source_key: source,
            input_identity: id,
            records: &[record],
            metadata: Some(PublicationMetadata::Analysis {
                execution_id: id,
                document: &doc,
            }),
            deadline: None,
        })
    };
    publish("analysis:b", "b1", vec![raw.clone()]).unwrap();
    publish("analysis:a", "a1", vec!["b1".into()]).unwrap();
    assert_eq!(
        store.active_result_ids("run-1", "analysis:b").unwrap(),
        vec![raw.clone()]
    );
    publish("analysis:a", "a2", vec!["b1".into()]).unwrap();
    assert_eq!(
        store
            .selected_result_id("run-1", "analysis:b")
            .unwrap()
            .as_deref(),
        Some("b1")
    );
    publish("analysis:b", "b2", vec![raw]).unwrap();
    assert!(
        store
            .selected_result_id("run-1", "analysis:a")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .records_for_result_ids("run-1", &["b2".into()])
            .unwrap()[0]["id"],
        "b2"
    );
    assert!(publish("analysis:a", "a2", vec!["b1".into()]).is_err());
    assert_eq!(store.records("run-1").unwrap().len(), 2);
    assert_eq!(store.records_history("run-1").unwrap().len(), 5);
}

#[test]
fn persisted_cancellation_wins_the_publication_race_and_owned_documents_are_removed() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let doc = json!({"run_id":"run-1","status":"running"});
    store
        .save_document("analysis-results", "attempt", &doc)
        .unwrap();
    let other_client = Store::open(root.path()).unwrap();
    assert_eq!(
        other_client.request_analysis_cancel("attempt").unwrap()["cancellation_requested"],
        true
    );
    assert!(
        store
            .publish_result(PublicationRequest {
                run_id: "run-1",
                source_key: "analysis:a",
                input_identity: "a1",
                records: &[event("a")],
                metadata: Some(PublicationMetadata::Analysis {
                    execution_id: "attempt",
                    document: &json!({"run_id":"run-1","status":"accepted"}),
                }),
                deadline: None,
            })
            .is_err()
    );
    assert!(store.records("run-1").unwrap().is_empty());
    assert!(store.remove_run("run-1").is_err());
    store
        .save_document(
            "analysis-results",
            "attempt",
            &json!({"run_id":"run-1","status":"cancelled"}),
        )
        .unwrap();
    assert_eq!(
        other_client.request_analysis_cancel("attempt").unwrap()["status"],
        "cancelled"
    );
    store
        .save_document("cancellations", "run-1", &json!({"run_id":"run-1"}))
        .unwrap();
    let capture = root.path().join("captures/run-1");
    let attempt = root.path().join("analysis-executions/attempt");
    std::fs::create_dir_all(&capture).unwrap();
    std::fs::create_dir_all(&attempt).unwrap();
    std::fs::write(capture.join("raw.txt"), "raw").unwrap();
    std::fs::write(attempt.join("input.json"), "{}").unwrap();
    store.remove_run("run-1").unwrap();
    assert!(store.document("analysis-results", "attempt").is_err());
    assert!(store.document("cancellations", "run-1").is_err());
    assert!(!capture.exists());
    assert!(!attempt.exists());
}

#[test]
fn concurrent_cancellation_and_publication_have_one_serialized_outcome() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    store
        .save_document(
            "analysis-results",
            "raced",
            &json!({"run_id":"run-1","status":"running"}),
        )
        .unwrap();
    let other = Store::open(root.path()).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let ready = barrier.clone();
    let publishing = store.clone();
    let publish = std::thread::spawn(move || {
        let records: Vec<_> = (0..256)
            .map(|index| event(&format!("raced-{index}")))
            .collect();
        ready.wait();
        publishing.publish_result(PublicationRequest {
            run_id: "run-1",
            source_key: "analysis:raced",
            input_identity: "raced-input",
            records: &records,
            metadata: Some(PublicationMetadata::Analysis {
                execution_id: "raced",
                document: &json!({"run_id":"run-1","status":"accepted"}),
            }),
            deadline: None,
        })
    });
    barrier.wait();
    let cancellation = other.request_analysis_cancel("raced").unwrap();
    let outcome = publish.join().unwrap();
    let document = store.document("analysis-results", "raced").unwrap();
    match outcome {
        Ok(accepted) => {
            assert!(accepted);
            assert_eq!(cancellation["status"], "accepted");
            assert_eq!(document["status"], "accepted");
            assert_eq!(store.records("run-1").unwrap().len(), 256);
        }
        Err(error) => {
            assert!(
                error.to_string().contains("cancelled before publication"),
                "{error:#}"
            );
            assert_eq!(cancellation["cancellation_requested"], true);
            assert_eq!(document["cancellation_requested"], true);
            assert!(store.records("run-1").unwrap().is_empty());
        }
    }
}

#[test]
fn opaque_observation_and_result_identities_keep_distinct_reference_roles_and_replay_bytes() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store
        .save_run(&Run {
            id: "recording".into(),
            name: "identity boundary".into(),
            status: "completed".into(),
            command: vec![],
            cwd: "/fixture".into(),
            runtime: "fixture".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: Some(0),
            diagnostics: vec![],
            source: json!({}),
            correctness: json!({}),
        })
        .unwrap();
    let event = json!({"schema_version":1,"kind":"event","name":"identity fixture","id":"observed:观测/α","run_id":"recording","producer_id":"fixture","pid":1,"tid":"thread","timestamp_us":1,"basis":"observed","attributes":{"arbitrary":{"nested":[1,null,"retained"]}}});
    let result = "result:原始/α";
    let document = json!({"id":result,"run_id":"recording","status":"accepted"});
    assert!(
        store
            .publish_result(PublicationRequest {
                run_id: "recording",
                source_key: "opaque-source",
                input_identity: "first-input",
                records: std::slice::from_ref(&event),
                metadata: Some(PublicationMetadata::Analysis {
                    execution_id: result,
                    document: &document,
                }),
                deadline: None,
            })
            .unwrap()
    );
    assert_eq!(store.records("recording").unwrap(), vec![event.clone()]);
    let connection = rusqlite::Connection::open(store.root().join("hunin.sqlite")).unwrap();
    let raw: String = connection
        .query_row(
            "SELECT record FROM observations WHERE id=?1",
            [event["id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw, serde_json::to_string(&event).unwrap());
    assert_eq!(
        store.active_result_ids("recording", "").unwrap(),
        vec![result.to_owned()]
    );

    let relation = json!({"schema_version":1,"kind":"relation","id":"derived:关系/β","run_id":"recording","producer_id":"analyzer","pid":0,"tid":"","timestamp_us":1,"basis":"derived","from":event["id"],"to":event["id"],"relation":"explains","evidence":[event["id"]]});
    let mut wrong_observation = relation.clone();
    wrong_observation["from"] = json!(result);
    let error = store
        .accept("recording", "wrong-observation", &[wrong_observation])
        .unwrap_err();
    assert_eq!(error.to_string(), format!("unresolved from: {result}"));
    let wrong_result = json!({"id":"failed","run_id":"recording","status":"accepted","input_result_sets":[event["id"]]});
    let error = store
        .publish_result(PublicationRequest {
            run_id: "recording",
            source_key: "derived",
            input_identity: "wrong-result",
            records: std::slice::from_ref(&relation),
            metadata: Some(PublicationMetadata::Analysis {
                execution_id: "failed",
                document: &wrong_result,
            }),
            deadline: None,
        })
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "analysis input changed before publication; rerun with current selected results"
    );
    assert_eq!(store.records("recording").unwrap(), vec![event.clone()]);

    let derived_id = "result:派生/β";
    let derived = json!({"id":derived_id,"run_id":"recording","status":"accepted","input_result_sets":[result]});
    assert!(
        store
            .publish_result(PublicationRequest {
                run_id: "recording",
                source_key: "derived",
                input_identity: "derived-input",
                records: std::slice::from_ref(&relation),
                metadata: Some(PublicationMetadata::Analysis {
                    execution_id: derived_id,
                    document: &derived,
                }),
                deadline: None,
            })
            .unwrap()
    );
    let replay_id = "attempt:replayed/γ";
    let replay = json!({"id":replay_id,"run_id":"recording","status":"accepted","input_result_sets":[result]});
    assert!(
        !store
            .publish_result(PublicationRequest {
                run_id: "recording",
                source_key: "derived",
                input_identity: "derived-input",
                records: std::slice::from_ref(&relation),
                metadata: Some(PublicationMetadata::Analysis {
                    execution_id: replay_id,
                    document: &replay,
                }),
                deadline: None,
            })
            .unwrap()
    );
    let replay = store.document("analysis-results", replay_id).unwrap();
    assert_eq!(replay["result_set_id"], derived_id);
    assert_eq!(replay["status"], "replayed");
    assert_eq!(store.records("recording").unwrap(), vec![event, relation]);
    let dependency: String = connection
        .query_row(
            "SELECT input_result_set_id FROM result_set_dependencies WHERE result_set_id=?1",
            [derived_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(dependency, result);
}

#[test]
fn deadline_expiring_inside_ownership_walk_rolls_back_selection_and_metadata() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store.save_run(&run()).unwrap();
    let original = event("before");
    store
        .accept_import("run-1", "source", "first", std::slice::from_ref(&original))
        .unwrap();
    let selected = store.selected_result_id("run-1", "source").unwrap();
    let document = json!({"run_id":"run-1","status":"running"});
    store
        .save_document("analysis-results", "attempt", &document)
        .unwrap();
    let records: Vec<_> = (0..128)
        .map(|index| {
            let mut record = event(&format!("span-{index}"));
            record["kind"] = json!("span");
            record["duration_us"] = json!(1);
            if index > 0 {
                record["parent_id"] = json!(format!("span-{}", index - 1));
            }
            record
        })
        .collect();
    let mut checkpoints = 0;
    let error = store
        .publish_result_checked(
            PublicationRequest {
                run_id: "run-1",
                source_key: "source",
                input_identity: "second",
                records: &records,
                metadata: Some(PublicationMetadata::Analysis {
                    execution_id: "attempt",
                    document: &json!({"run_id":"run-1","status":"accepted"}),
                }),
                deadline: None,
            },
            || {
                checkpoints += 1;
                // One initial transaction check and the proposed-node registration pass
                // precede the graph walk. Expire deterministically within that walk.
                if checkpoints >= 1 + records.len() + 32 {
                    publication::check_deadline(Some(
                        std::time::Instant::now() - Duration::from_secs(1),
                    ))?;
                }
                Ok(())
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("deadline"), "{error}");
    assert_eq!(checkpoints, 1 + records.len() + 32);
    assert_eq!(
        store.selected_result_id("run-1", "source").unwrap(),
        selected
    );
    assert_eq!(store.records("run-1").unwrap(), vec![original]);
    assert_eq!(
        store.document("analysis-results", "attempt").unwrap(),
        document
    );
    assert_eq!(store.records_history("run-1").unwrap().len(), 1);
}

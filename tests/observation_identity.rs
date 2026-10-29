use hunin::{model::runs::Run, storage::Store};
use serde_json::json;

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
            .publish(
                "recording",
                "opaque-source",
                "first-input",
                std::slice::from_ref(&event),
                Some(("analysis-results", result, &document))
            )
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
        .publish(
            "recording",
            "derived",
            "wrong-result",
            std::slice::from_ref(&relation),
            Some(("analysis-results", "failed", &wrong_result)),
        )
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
            .publish(
                "recording",
                "derived",
                "derived-input",
                std::slice::from_ref(&relation),
                Some(("analysis-results", derived_id, &derived))
            )
            .unwrap()
    );
    let replay_id = "attempt:replayed/γ";
    let replay = json!({"id":replay_id,"run_id":"recording","status":"accepted","input_result_sets":[result]});
    assert!(
        !store
            .publish(
                "recording",
                "derived",
                "derived-input",
                std::slice::from_ref(&relation),
                Some(("analysis-results", replay_id, &replay))
            )
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

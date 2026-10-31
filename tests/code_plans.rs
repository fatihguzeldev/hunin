use hunin::{
    model::runs::Run,
    operations::{Operations, catalog},
    storage::Store,
};
use serde_json::{Value, json};

fn fixture() -> (tempfile::TempDir, Store, Operations) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let run = Run {
        id: "plan-run".into(),
        name: "read-only plan".into(),
        status: "completed".into(),
        command: vec!["must-not-launch".into()],
        cwd: root.path().display().to_string(),
        runtime: "none".into(),
        started_at_us: 1,
        finished_at_us: Some(5),
        exit_code: Some(0),
        diagnostics: vec![],
        source: json!({"fingerprint":"source-v1"}),
        correctness: json!({"passed":true}),
    };
    store.save_run(&run).unwrap();
    store.accept(&run.id, "plan-fixture", &[json!({"schema_version":1,"kind":"sample","id":"sample","run_id":run.id,"producer_id":"fixture","pid":1,"tid":"main","timestamp_us":2,"basis":"sampled","stack":[{"function":"work"}],"weight":1,"weight_unit":"samples"})]).unwrap();
    let operations = Operations::new(store.clone());
    (root, store, operations)
}
fn validate(value: &Value) {
    let definition = catalog()
        .iter()
        .find(|op| op["name"] == "code.plan")
        .unwrap();
    jsonschema::validator_for(&definition["outputSchema"])
        .unwrap()
        .validate(value)
        .unwrap();
    for step in value["steps"].as_array().unwrap() {
        if let Some(operation) = step["operation"].as_str() {
            let action = catalog()
                .iter()
                .find(|item| item["name"] == operation)
                .unwrap();
            jsonschema::validator_for(&action["inputSchema"])
                .unwrap()
                .validate(&step["params"])
                .unwrap();
        }
    }
}
#[tokio::test]
async fn investigation_plan_is_read_only_and_reports_unavailable_capture() {
    let (_root, store, operations) = fixture();
    let before = store.records("plan-run").unwrap();
    let plan = operations
        .call(
            "code.plan",
            json!({"run_id":"plan-run","observation_id":"sample","goal":"semantic_origin"}),
        )
        .await
        .unwrap();
    validate(&plan);
    assert!(plan["result_identity"].is_string());
    assert!(
        plan["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["id"] == "code-capture-unavailable" && step["status"] == "blocked")
    );
    assert_eq!(store.records("plan-run").unwrap(), before);
    assert!(store.documents("analysis-results").unwrap().is_empty());
    assert_eq!(store.runs(100, 0).unwrap().len(), 1);
}
#[tokio::test]
async fn mapper_plans_keep_declared_input_applicability_explicit() {
    let (_root, store, operations) = fixture();
    for (id, fingerprint) in [("matching", "source-v1"), ("different", "another-source")] {
        store.save_document("analyses",id,&json!({"id":id,"name":id,"output_contract":{"name":"code-mapping","version":1},"requirements":{"runtime":"none","source_fingerprint":fingerprint}})).unwrap();
    }
    let plan = operations
        .call(
            "code.plan",
            json!({"run_id":"plan-run","observation_id":"sample"}),
        )
        .await
        .unwrap();
    validate(&plan);
    let steps = plan["steps"].as_array().unwrap();
    let matching = steps
        .iter()
        .find(|step| step["id"] == "mapper:matching")
        .unwrap();
    assert_eq!(matching["operation"], "analyses.run");
    assert_eq!(matching["params"]["analysis_id"], "matching");
    let wrong = steps
        .iter()
        .find(|step| step["id"] == "mapper:different")
        .unwrap();
    assert_eq!(wrong["status"], "blocked");
    assert!(wrong["operation"].is_null());
}
#[tokio::test]
async fn missing_observation_never_proposes_a_workload_execution() {
    let (_root, _store, operations) = fixture();
    let plan = operations
        .call(
            "code.plan",
            json!({"run_id":"plan-run","observation_id":"absent"}),
        )
        .await
        .unwrap();
    validate(&plan);
    assert!(plan["anchor"].is_null());
    assert!(
        plan["steps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|step| step["status"] == "blocked" && step["operation"].is_null())
    );
}

#[tokio::test]
async fn node_capture_plan_does_not_execute_retained_launch_metadata() {
    let (root, store, operations) = fixture();
    let mut run = store.run("plan-run").unwrap();
    run.runtime = "node".into();
    // A read must not probe this path, even when the recording calls it Node.
    run.source["launch_executable"] = json!(root.path().join("untrusted-node"));
    run.source["capture_policy"] = json!({"mode":"code-origin","code_options":{"compiler_export":true,"function_filter":"guest.run*","wasm_function_index":16,"sampling_interval_us":500,"max_artifact_bytes":100663296}});
    store.save_run(&run).unwrap();
    let plan = operations
        .call(
            "code.plan",
            json!({"run_id":"plan-run","observation_id":"sample"}),
        )
        .await
        .unwrap();
    validate(&plan);
    let capture = plan["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["id"] == "capture-code-origins")
        .unwrap();
    assert_eq!(capture["operation"], "runs.start");
    assert_eq!(
        capture["params"]["command"][0],
        run.source["launch_executable"]
    );
    assert_eq!(
        capture["params"]["code_options"],
        run.source["capture_policy"]["code_options"]
    );
    let source = operations
        .call(
            "code.plan",
            json!({"run_id":"plan-run","observation_id":"sample","goal":"source"}),
        )
        .await
        .unwrap();
    validate(&source);
    let source = source["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["id"] == "capture-code-origins")
        .unwrap();
    assert_eq!(source["params"]["code_options"]["compiler_export"], false);
    assert!(
        source["params"]["code_options"]
            .get("wasm_function_index")
            .is_none()
    );
    assert_eq!(store.runs(100, 0).unwrap().len(), 1);
}

#[tokio::test]
async fn truncated_package_inventory_does_not_claim_no_mapper_is_registered() {
    let (_root, store, operations) = fixture();
    for index in 0..4 {
        let id = format!("a-{index}");
        store
            .save_document("analyses", &id, &json!({"id":id,"name":id}))
            .unwrap();
    }
    store
        .save_document(
            "analyses",
            "z-mapper",
            &json!({"id":"z-mapper","output_contract":{"name":"code-mapping","version":1}}),
        )
        .unwrap();
    let plan = operations
        .call(
            "code.plan",
            json!({"run_id":"plan-run","observation_id":"sample","limit":3}),
        )
        .await
        .unwrap();
    validate(&plan);
    assert_eq!(plan["truncated"], true);
    let unavailable = plan["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["id"] == "mapper-unavailable")
        .unwrap();
    assert!(
        unavailable["reason"]
            .as_str()
            .unwrap()
            .contains("inventory is truncated")
    );
    assert!(
        !unavailable["reason"]
            .as_str()
            .unwrap()
            .contains("is registered")
    );
}

#[tokio::test]
async fn proof_plan_keeps_distinct_byte_ranges_in_the_same_artifact() {
    let (_root, store, operations) = fixture();
    let artifact = store
        .add_artifact_bytes(
            "plan-run",
            "code.bin",
            "control",
            &(0u8..32).collect::<Vec<_>>(),
        )
        .unwrap();
    let proof = json!({"artifact_id":artifact.id,"sha256":artifact.sha256});
    let range = |start, end| json!({"space":"function_offset","start":format!("0x{start:x}"),"end_exclusive":format!("0x{end:x}"),"function_index":0});
    let instance = |id: &str, representation: &str, range: Value, artifact: Value| json!({"schema_version":1,"kind":"event","id":id,"run_id":"plan-run","producer_id":"fixture","pid":1,"tid":"main","timestamp_us":2,"basis":"observed","name":"code.instance","attributes":{"code_instance":{"schema_version":1,"instance_kind":"static","representation":representation,"range":range,"artifact":artifact}}});
    let mut records = vec![
        instance("wasm", "wasm", range(0, 32), proof.clone()),
        instance("logical-a", "logical", range(0, 1), Value::Null),
        instance("logical-b", "logical", range(0, 1), Value::Null),
    ];
    for (index, offset) in [0, 8].into_iter().enumerate() {
        let mut scoped = proof.clone();
        scoped["offset"] = json!(offset);
        scoped["length"] = json!(4);
        records.push(json!({"schema_version":1,"kind":"relation","id":format!("mapping-{index}"),"run_id":"plan-run","producer_id":"fixture","pid":1,"tid":"main","timestamp_us":2,"basis":"derived","from":"wasm","to":if index==0{"logical-a"}else{"logical-b"},"relation":"code_maps_to","evidence":["sample"],"attributes":{"code_mapping":{"schema_version":1,"from_range":range(offset,offset+4),"to_range":range(0,1),"cardinality":"exclusive","proof":{"artifact_refs":[scoped],"method":"fixture-scoped-origin","verification":"identity_checked"}}}}));
    }
    store.accept("plan-run", "proof-fixture", &records).unwrap();
    let plan = operations
        .call(
            "code.plan",
            json!({"run_id":"plan-run","observation_id":"wasm"}),
        )
        .await
        .unwrap();
    validate(&plan);
    let reads = plan["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|step| step["operation"] == "artifacts.read")
        .collect::<Vec<_>>();
    assert!(
        reads
            .iter()
            .any(|step| step["params"]["offset"] == 0 && step["params"]["limit"] == 4)
    );
    assert!(
        reads
            .iter()
            .any(|step| step["params"]["offset"] == 8 && step["params"]["limit"] == 4)
    );
    assert!(reads.iter().all(|step| step["params"]["encoding"] == "hex"));
    let ids = reads
        .iter()
        .map(|step| step["id"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), reads.len());
}

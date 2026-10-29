use hunin::{
    model::runs::Run,
    operations::{Operations, catalog},
    storage::Store,
};
use serde_json::{Value, json};

fn validate_result(name: &str, result: &Value) {
    let definition = catalog()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap();
    jsonschema::validator_for(&definition["outputSchema"])
        .unwrap()
        .validate(result)
        .unwrap_or_else(|error| panic!("{name} response violates its output contract: {error}"));
}

fn validate_transport(name: &str, result: &Value) {
    let definitions: Value =
        serde_json::from_str(include_str!("../protocol/response.schema.json")).unwrap();
    jsonschema::validator_for(&definitions["$defs"][name])
        .unwrap()
        .validate(result)
        .unwrap();
}

#[test]
fn published_output_schemas_resolve_without_external_files_or_network() {
    for operation in catalog() {
        if let Some(schema) = operation.get("outputSchema") {
            jsonschema::validator_for(schema)
                .unwrap_or_else(|error| panic!("{}: {error}", operation["name"]));
        }
    }
}

#[tokio::test]
async fn run_contract_preserves_extension_status_null_options_and_arbitrary_metadata() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    let run = Run {
        id: "legacy".into(),
        name: "legacy".into(),
        status: "archived-by-extension".into(),
        command: vec![],
        cwd: data.path().to_string_lossy().into_owned(),
        runtime: "imported".into(),
        started_at_us: 1,
        finished_at_us: None,
        exit_code: None,
        diagnostics: vec![],
        source: Value::Null,
        correctness: json!("unverified"),
    };
    store.save_run(&run).unwrap();
    let operations = Operations::new(store);
    let result = operations
        .call("runs.get", json!({"run_id":"legacy"}))
        .await
        .unwrap();
    validate_result("runs.get", &result);
    assert_eq!(result["run"], serde_json::to_value(&run).unwrap());
    let page = operations.call("runs.list", json!({})).await.unwrap();
    validate_result("runs.list", &page);
    assert_eq!(page["next_cursor"], Value::Null);

    let success = hunin::interfaces::mcp::handle(&operations, &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"runs.get","arguments":{"run_id":"legacy"}}}), true).await;
    validate_transport("McpToolSuccess", &success["result"]);
    validate_result("runs.get", &success["result"]["structuredContent"]);
    let failure = hunin::interfaces::mcp::handle(&operations, &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"runs.get","arguments":{"run_id":"missing"}}}), true).await;
    validate_transport("McpToolFailure", &failure["result"]);
    assert!(failure["result"].get("structuredContent").is_none());
}

#[test]
fn summary_contract_retains_non_finite_aggregate_as_null() {
    let sample =
        |id: &str| json!({"kind":"sample","id":id,"weight":f64::MAX,"weight_unit":"samples"});
    let summary = hunin::query::summary(&[sample("one"), sample("two")]);
    assert_eq!(summary["total_weight"], Value::Null);
    let definitions: Value =
        serde_json::from_str(include_str!("../protocol/response.schema.json")).unwrap();
    jsonschema::validator_for(&definitions["$defs"]["Summary"])
        .unwrap()
        .validate(&summary)
        .unwrap();
}

#[tokio::test]
async fn capabilities_and_source_contracts_cover_real_discovery_and_unverified_files() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let source = project.path().join("control.js");
    std::fs::write(&source, "function currentControl() {}\n").unwrap();
    let store = Store::open(data.path()).unwrap();
    store
        .save_run(&Run {
            id: "source".into(),
            name: "source".into(),
            status: "completed".into(),
            command: vec![],
            cwd: project.path().to_string_lossy().into_owned(),
            runtime: "imported".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: None,
            diagnostics: vec![],
            source: Value::Null,
            correctness: Value::Null,
        })
        .unwrap();
    let operations = Operations::new(store);
    let result = operations
        .call("source.read", json!({"run_id":"source","path":source}))
        .await
        .unwrap();
    validate_result("source.read", &result);
    assert_eq!(result["identity_verified"], false);
    assert_eq!(result["source_kind"], "current_file");
    let discovery = operations.call("capabilities", json!({})).await.unwrap();
    validate_result("capabilities", &discovery);
    assert_eq!(
        discovery["verification"],
        "Discovery never labels executable presence as a verified CPU recording."
    );
}

#[tokio::test]
async fn metadata_contracts_preserve_legacy_documents_and_new_operation_results() {
    let data = tempfile::tempdir().unwrap();
    let package = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store
        .save_run(&Run {
            id: "recording".into(),
            name: "recording".into(),
            status: "completed".into(),
            command: vec![],
            cwd: package.path().to_string_lossy().into_owned(),
            runtime: "fixture".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: None,
            diagnostics: vec![],
            source: json!({"fingerprint":"known"}),
            correctness: Value::Null,
        })
        .unwrap();
    for namespace in ["analyses", "scenarios"] {
        store
            .save_document(
                namespace,
                "legacy",
                &json!({"id":"legacy","name":"retained","created_at_us":0}),
            )
            .unwrap();
        store
            .save_document(namespace, "scalar", &json!("historical scalar"))
            .unwrap();
    }
    store
        .save_document(
            "analysis-results",
            "legacy",
            &json!({"id":"legacy","run_id":"recording","analysis_id":"legacy","status":"failed"}),
        )
        .unwrap();
    store.accept("recording", "contract-input", &[json!({"schema_version":1,"kind":"event","id":"input","run_id":"recording","producer_id":"control","pid":0,"tid":"","timestamp_us":1,"basis":"observed","name":"input"})]).unwrap();
    let operations = Operations::new(store);
    for operation in ["analyses.list", "scenarios.list"] {
        let page = operations.call(operation, json!({})).await.unwrap();
        validate_result(operation, &page);
        assert!(page.as_object().unwrap().values().any(|value| {
            value
                .as_array()
                .is_some_and(|rows| rows.contains(&json!("historical scalar")))
        }));
    }
    let saved = operations
        .call(
            "scenarios.save",
            json!({"name":"minimal","command":["workload"]}),
        )
        .await
        .unwrap();
    validate_result("scenarios.save", &saved);
    assert!(saved["scenario"].get("cwd").is_none());
    assert!(saved["scenario"].get("timeout_seconds").is_none());

    std::fs::write(package.path().join("analyze.py"), "import json,sys\nvalue=json.load(sys.stdin)\nprint(json.dumps({'schema_version':1,'kind':'event','id':'contract-event','run_id':value['run']['id'],'producer_id':'contract','pid':0,'tid':'','timestamp_us':1,'basis':'derived','name':'contract','evidence':[value['records'][0]['id']]}))\n").unwrap();
    let python = std::env::var("HUNIN_TEST_PYTHON").unwrap_or_else(|_| {
        if cfg!(windows) {
            "python".into()
        } else {
            "python3".into()
        }
    });
    let registered = operations.call("analyses.register", json!({"name":"contract","command":[python,"analyze.py"],"cwd":package.path(),"requirements":{"runtime":"fixture","source_fingerprint":"known"}})).await.unwrap();
    validate_result("analyses.register", &registered);
    let started = operations.call("analyses.run", json!({"run_id":"recording","analysis_id":registered["analysis"]["id"],"timeout_seconds":5})).await.unwrap();
    validate_result("analyses.run", &started);
    let execution = started["result_set_id"].as_str().unwrap();
    let finished = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        operations.analyses.wait(execution),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(finished["status"], "accepted", "{finished}");
    let results = operations
        .call("analyses.results", json!({"run_id":"recording"}))
        .await
        .unwrap();
    validate_result("analyses.results", &results);
    assert_eq!(results["results"].as_array().unwrap().len(), 2);
    let canceled = operations
        .call("analyses.cancel", json!({"execution_id":execution}))
        .await
        .unwrap();
    validate_result("analyses.cancel", &canceled);
    assert_eq!(canceled["status"], "accepted");
}

#[test]
fn observation_response_guards_match_the_shared_schema_only_controls() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../protocol/fixtures/record-response-cases.json"
    ))
    .unwrap();
    let schema = &catalog()
        .iter()
        .find(|entry| entry["name"] == "evidence.get")
        .unwrap()["outputSchema"];
    let validator = jsonschema::validator_for(schema).unwrap();
    for fixture in fixtures["cases"].as_array().unwrap() {
        assert_eq!(
            validator.is_valid(&json!({"records":[fixture["record"]]})),
            fixture["valid"].as_bool().unwrap(),
            "{}",
            fixture["name"]
        );
    }
}

#[tokio::test]
async fn query_contracts_cover_nullable_positions_clock_metadata_and_shared_comparison() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    for (id, weight) in [("before", 3), ("after", 2)] {
        store.save_run(&Run {
            id: id.into(), name: id.into(), status: "completed".into(), command: vec!["workload".into()],
            cwd: data.path().to_string_lossy().into_owned(), runtime: "fixture".into(), started_at_us: 10,
            finished_at_us: Some(20), exit_code: Some(0), diagnostics: vec![],
            source: json!({"fingerprint":"known","collector_settings_fingerprint":"settings","environment_fingerprint":"environment","workload_inputs":{"fingerprint":"inputs"},"correctness_recipe_fingerprint":"recipe","supervised_elapsed_us":10,"supervised_execution_scope":"target_process"}),
            correctness: json!({"passed":true}),
        }).unwrap();
        let record = |kind: &str, suffix: &str, basis: &str| json!({"schema_version":1,"kind":kind,"id":format!("{id}-{suffix}"),"run_id":id,"producer_id":"control","pid":1,"tid":"thread","timestamp_us":11,"basis":basis});
        let mut context = record("context", "context", "explicit");
        context["name"] = json!("context");
        let mut sample = record("sample", "sample", "sampled");
        sample["stack"] = json!([{"function":"hotspot","file":"missing.rs"}]);
        sample["weight"] = json!(weight);
        sample["weight_unit"] = json!("samples");
        sample["context_id"] = context["id"].clone();
        let mut metric = record("metric", "metric", "observed");
        metric["name"] = json!("process.cpu_time");
        metric["value"] = json!(1);
        metric["unit"] = json!("milliseconds");
        metric["scope"] = json!("process");
        metric["attributes"] = json!({"clock_alignment":"unmapped","clock":{"domain":"opaque"},"process_role":{"extension":"unknown"}});
        let mut relation = record("relation", "relation", "explicit");
        relation["from"] = context["id"].clone();
        relation["to"] = sample["id"].clone();
        relation["relation"] = json!("dispatches");
        relation["evidence"] = json!([context["id"]]);
        store
            .accept(id, "contract-input", &[context, sample, metric, relation])
            .unwrap();
    }
    let operations = Operations::new(store);
    for name in [
        "profiles.hotspots",
        "profiles.tree",
        "timeline.query",
        "resources.query",
        "contexts.query",
    ] {
        let result = operations
            .call(name, json!({"run_id":"before"}))
            .await
            .unwrap();
        validate_result(name, &result);
        if name == "profiles.hotspots" {
            assert_eq!(result["hotspots"][0]["line"], Value::Null);
        }
        if name == "profiles.tree" {
            assert_eq!(result["roots"][0]["line"], Value::Null);
        }
        if name == "resources.query" {
            assert!(result["series"][0]["process_role"].is_object());
        }
    }
    let evidence = operations
        .call(
            "evidence.get",
            json!({"run_id":"before","ids":["before-sample"]}),
        )
        .await
        .unwrap();
    validate_result("evidence.get", &evidence);
    let comparison = operations
        .call(
            "comparisons.create",
            json!({"before":"before","after":"after"}),
        )
        .await
        .unwrap();
    validate_result("comparisons.create", &comparison);
    assert_eq!(comparison["comparable"], true);
}

#[tokio::test]
async fn artifact_contracts_cover_real_pages_and_lossy_byte_excerpts() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    store
        .save_run(&Run {
            id: "recording".into(),
            name: "recording".into(),
            status: "completed".into(),
            command: vec![],
            cwd: data.path().to_string_lossy().into_owned(),
            runtime: "imported".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: None,
            diagnostics: vec![],
            source: json!({"fingerprint":"unknown"}),
            correctness: json!({"passed":null}),
        })
        .unwrap();
    let artifact = store
        .add_artifact_bytes("recording", "utf8", "control", "AéB".as_bytes())
        .unwrap();
    store
        .add_artifact_bytes("recording", "other", "control", b"other")
        .unwrap();
    let operations = Operations::new(store);

    let page = operations
        .call("artifacts.list", json!({"run_id":"recording","limit":1}))
        .await
        .unwrap();
    validate_result("artifacts.list", &page);
    assert_eq!(page["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(page["next_cursor"], "1");
    let last = operations
        .call(
            "artifacts.list",
            json!({"run_id":"recording","limit":1,"cursor":"1"}),
        )
        .await
        .unwrap();
    validate_result("artifacts.list", &last);
    assert_eq!(last["next_cursor"], Value::Null);

    let excerpt = operations
        .call(
            "artifacts.read",
            json!({"run_id":"recording","artifact_id":artifact.id,"offset":2,"limit":1}),
        )
        .await
        .unwrap();
    validate_result("artifacts.read", &excerpt);
    assert_eq!(excerpt["content"], "�");
    assert_eq!(excerpt["encoding"], "utf8_lossy");
    assert_eq!(excerpt["next_offset"], 3);
    assert_eq!(excerpt["size"], 4);
    let end = operations
        .call(
            "artifacts.read",
            json!({"run_id":"recording","artifact_id":artifact.id,"offset":4}),
        )
        .await
        .unwrap();
    validate_result("artifacts.read", &end);
    assert_eq!(end["content"], "");
    assert_eq!(end["next_offset"], Value::Null);
}

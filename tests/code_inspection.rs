use hunin::{model::runs::Run, operations::Operations, storage::Store};
use serde_json::{Value, json};

fn fixture() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store
        .save_run(&Run {
            id: "r".into(),
            name: "code oracle".into(),
            status: "completed".into(),
            command: vec![],
            cwd: "/fixture".into(),
            runtime: "node".into(),
            started_at_us: 0,
            finished_at_us: Some(40),
            exit_code: Some(0),
            diagnostics: vec![],
            source: json!({}),
            correctness: json!({}),
        })
        .unwrap();
    let event = |id: &str, name: &str, attributes: Value| json!({"schema_version":1,"kind":"event","id":id,"run_id":"r","producer_id":"fixture","pid":7,"tid":"main","timestamp_us":10,"basis":"observed","name":name,"attributes":attributes});
    let range = |space: &str, start: &str, end: &str, index: Option<u64>| json!({"space":space,"start":start,"end_exclusive":end,"function_index":index});
    let validity = json!({"clock":"unix_epoch","alignment":"direct_epoch","start_us":10,"end_us":40,"start_censored":false,"end_censored":false,"boundary_basis":"runtime_code_events"});
    let mut rows = vec![event(
        "instance",
        "code.instance",
        json!({"code_instance":{"schema_version":1,"instance_kind":"runtime","runtime_instance_id":"process7/isolate2","representation":"machine_code","architecture":"aarch64","range":range("runtime_address","0x1000","0x1040",None),"validity":validity}}),
    )];
    for id in ["target-a", "target-b"] {
        rows.push(event(id,"code.instance",json!({"code_instance":{"schema_version":1,"instance_kind":"static","representation":"wasm","range":range("function_offset","0x0","0x20",Some(17))}})));
    }
    for (id, time, weight, unit, offset, aligned) in [
        ("s1", 10, 2, "samples", "0x1000", true),
        ("s2", 20, 3, "samples", "0x1010", true),
        ("s3", 30, 5, "samples", "0x1020", true),
        ("bytes", 20, 7, "bytes", "0x1000", true),
        ("cpu", 20, 11, "samples", "0x1000", false),
    ] {
        let frame = json!({"function":"generated","code_instance_id":"instance","code_offset":offset,"address":offset});
        rows.push(json!({"schema_version":1,"kind":"sample","id":id,"run_id":"r","producer_id":"fixture","pid":7,"tid":"main","timestamp_us":time,"basis":"sampled","weight":weight,"weight_unit":unit,"stack":[frame.clone(),frame],"attributes":{"clock":if aligned{"unix_epoch"}else{"thread_cpu"},"clock_alignment":if aligned{"direct_epoch"}else{"unmapped"}}}));
    }
    for (id, target, cardinality) in [
        ("map-a", "target-a", "exclusive"),
        ("map-b", "target-b", "shared"),
    ] {
        rows.push(json!({"schema_version":1,"kind":"relation","id":id,"run_id":"r","producer_id":"fixture","pid":7,"tid":"main","timestamp_us":10,"basis":"derived","from":"instance","to":target,"relation":"code_maps_to","evidence":["s1"],"attributes":{"code_mapping":{"schema_version":1,"from_range":range("runtime_address","0x1000","0x1010",None),"to_range":range("function_offset","0x0","0x10",Some(17)),"cardinality":cardinality,"validity":validity,"proof":{"method":"fixture_map","verification":"declared","artifact_refs":[]}}}}));
    }
    rows.push(event("unmapped","code.mapping_unavailable",json!({"code_mapping_unavailable":{"schema_version":1,"anchor":{"observation_id":"s2","frame_index":0},"state":"unmapped","reason_code":"no_exported_range"}})));
    store.accept("r", "code-oracle", &rows).unwrap();
    (root, store)
}

#[tokio::test]
async fn code_coverage_preserves_raw_unit_weights_recursion_and_shared_paths() {
    let (_root, store) = fixture();
    let identity = store.records_snapshot("r").unwrap().result_identity;
    let ops = Operations::new(store);
    let result=ops.call("code.inspect",json!({"run_id":"r","observation_id":"s1","frame_index":0,"unit":"samples","start_us":10,"end_us":30})).await.unwrap();
    assert_eq!(result["result_identity"], identity);
    assert_eq!(result["anchor"]["code_instance_id"], "instance");
    assert_eq!(
        result["coverage"]["total"],
        json!({"sample_count":3,"weight":10.0})
    );
    assert_eq!(
        result["coverage"]["shared"],
        json!({"sample_count":1,"weight":2.0})
    );
    assert_eq!(
        result["coverage"]["unmapped"],
        json!({"sample_count":1,"weight":3.0})
    );
    assert_eq!(
        result["coverage"]["unknown"],
        json!({"sample_count":1,"weight":5.0})
    );
    assert_eq!(result["coverage"]["unaligned_sample_count"], 1);
    assert_eq!(result["mappings"].as_array().unwrap().len(), 2);
    assert_eq!(result["instances"].as_array().unwrap().len(), 3);
    assert_eq!(result["truncated"], false);
    let operation = hunin::operations::catalog()
        .iter()
        .find(|o| o["name"] == "code.inspect")
        .unwrap();
    jsonschema::validator_for(&operation["outputSchema"])
        .unwrap()
        .validate(&result)
        .unwrap();
}

#[tokio::test]
async fn code_limits_and_absence_never_turn_missing_mapping_into_a_zero_claim() {
    let (_root, store) = fixture();
    let ops = Operations::new(store);
    let result = ops
        .call(
            "code.inspect",
            json!({"run_id":"r","observation_id":"s1","limit":1}),
        )
        .await
        .unwrap();
    assert_eq!(result["truncated"], true);
    assert_eq!(result["coverage"]["total"]["weight"], 10.0);
    let missing = ops
        .call(
            "code.inspect",
            json!({"run_id":"r","observation_id":"missing","unit":"count"}),
        )
        .await
        .unwrap();
    assert!(missing["anchor"].is_null());
    assert_eq!(missing["coverage"]["status"], "unavailable");
    assert!(missing["coverage"]["total"]["weight"].is_null());
}

#[tokio::test]
async fn transitive_code_ranges_keep_shared_intermediate_units_without_inventing_costs() {
    let (_root, store) = fixture();
    let unit = json!({"schema_version":1,"kind":"event","id":"semantic","run_id":"r","producer_id":"semantic-map","pid":7,"tid":"main","timestamp_us":10,"basis":"derived","name":"code.instance","evidence":["s1"],"attributes":{"code_instance":{"schema_version":1,"instance_kind":"static","representation":"logical","label":"exported operation","range":{"space":"artifact_offset","start":"0x0","end_exclusive":"0x8"}}}});
    let link = json!({"schema_version":1,"kind":"relation","id":"map-semantic","run_id":"r","producer_id":"semantic-map","pid":7,"tid":"main","timestamp_us":10,"basis":"derived","from":"target-a","to":"semantic","relation":"code_maps_to","evidence":["s1"],"attributes":{"code_mapping":{"schema_version":1,"from_range":{"space":"function_offset","start":"0x0","end_exclusive":"0x10","function_index":17},"to_range":{"space":"artifact_offset","start":"0x0","end_exclusive":"0x8"},"cardinality":"shared","proof":{"method":"declared_semantic_map","verification":"declared","artifact_refs":[]}}}});
    store.accept("r", "semantic-oracle", &[unit, link]).unwrap();
    let result = Operations::new(store)
        .call(
            "code.inspect",
            json!({"run_id":"r","observation_id":"s1","unit":"samples","start_us":10,"end_us":30}),
        )
        .await
        .unwrap();
    assert_eq!(result["instances"].as_array().unwrap().len(), 4);
    assert_eq!(result["mappings"].as_array().unwrap().len(), 3);
    assert_eq!(result["coverage"]["total"]["weight"], 10.0);
    assert_eq!(result["coverage"]["shared"]["weight"], 2.0);
    assert_eq!(result["coverage"]["basis"], "direct_code_mapping");
}

#[test]
fn code_reads_and_relation_walks_keep_existing_budget_and_cancellation_origins() {
    use hunin::query::{self, QueryLimits, RecordingQuery, WorkBudgetExceeded};
    let (_root, store) = fixture();
    let params = json!({"observation_id":"s1"});
    let limits = QueryLimits {
        maximum_frame_visits: 1,
        ..Default::default()
    };
    let error = query::read_recording(
        &store,
        "r",
        RecordingQuery::CodeInspect(&params),
        limits,
        || Ok(()),
    )
    .unwrap_err();
    assert!(error.is::<WorkBudgetExceeded>());
    let error = query::read_recording(
        &store,
        "r",
        RecordingQuery::CodeInspect(&params),
        QueryLimits::default(),
        || Err(anyhow::anyhow!("caller abandoned")),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "caller abandoned");
}

#[tokio::test]
async fn runtime_relative_code_facts_are_inspectable_only_in_the_explicit_same_origin() {
    let (_root, store) = fixture();
    let mut rows = store.records("r").unwrap();
    let mut run = store.run("r").unwrap();
    run.id = "relative".into();
    store.save_run(&run).unwrap();
    for record in &mut rows {
        record["run_id"] = json!("relative");
        if record["id"] == "cpu" {
            continue;
        }
        record["attributes"]["clock"] = json!("v8:pid7:profile-a");
        record["attributes"]["clock_alignment"] = json!("runtime_relative");
        for payload in ["code_instance", "code_mapping"] {
            if let Some(validity) = record["attributes"][payload].get_mut("validity") {
                validity["clock"] = json!("v8:pid7:profile-a");
                validity["alignment"] = json!("runtime_relative");
            }
        }
    }
    store.accept("relative", "relative-code", &rows).unwrap();
    let ops = Operations::new(store);
    let result=ops.call("code.inspect",json!({"run_id":"relative","observation_id":"s1","clock_domain":"v8:pid7:profile-a","start_us":10,"end_us":30})).await.unwrap();
    assert_eq!(result["coverage"]["total"]["weight"], 10.0);
    assert_eq!(result["coverage"]["shared"]["weight"], 2.0);
    assert_eq!(result["clock_domain"], "v8:pid7:profile-a");
    assert_eq!(result["anchor"]["aligned"], true);
    let epoch = ops
        .call(
            "code.inspect",
            json!({"run_id":"relative","observation_id":"s1"}),
        )
        .await
        .unwrap();
    assert_eq!(epoch["coverage"]["status"], "unavailable");
    assert!(epoch["coverage"]["total"]["weight"].is_null());
}

fn unbound_sample() -> Value {
    json!({"schema_version":1,"kind":"sample","id":"unbound","run_id":"r","producer_id":"raw","pid":7,"tid":"main","timestamp_us":15,"basis":"sampled","weight":13,"weight_unit":"samples","stack":[{"function":"opaque","address":"0x1000"}],"attributes":{"clock":"unix_epoch","clock_alignment":"direct_epoch"}})
}

fn frame_mapping(id: &str, sample: &str, target: &str, offset: &str) -> Value {
    json!({"schema_version":1,"kind":"relation","id":id,"run_id":"r","producer_id":"mapper","pid":7,"tid":"main","timestamp_us":15,"basis":"derived","from":sample,"to":target,"relation":"code_frame_maps_to","evidence":[sample],"attributes":{"code_frame_mapping":{"schema_version":1,"frame_index":0,"code_offset":offset,"code_address_basis":"retained_mapping_byte_coordinate","proof":{"method":"fixture_frame_map","verification":"declared","artifact_refs":[]}}}})
}

#[tokio::test]
async fn derived_frame_associations_preserve_raw_samples_and_reuse_unit_safe_mapping_coverage() {
    let (_root, store) = fixture();
    store
        .accept("r", "unbound-input", &[unbound_sample()])
        .unwrap();
    store
        .accept(
            "r",
            "frame-map",
            &[frame_mapping("binding", "unbound", "instance", "0x1000")],
        )
        .unwrap();
    let raw = store
        .records("r")
        .unwrap()
        .into_iter()
        .find(|record| record["id"] == "unbound")
        .unwrap();
    assert!(raw["stack"][0].get("code_instance_id").is_none());
    let result = Operations::new(store)
        .call(
            "code.inspect",
            json!({"run_id":"r","observation_id":"unbound"}),
        )
        .await
        .unwrap();
    assert_eq!(result["anchor"]["association_status"], "derived");
    assert_eq!(result["anchor"]["association_ids"], json!(["binding"]));
    assert_eq!(result["anchor"]["code_instance_id"], "instance");
    assert_eq!(result["anchor"]["code_offset"], "0x1000");
    assert_eq!(
        result["anchor"]["code_address_basis"],
        "retained_mapping_byte_coordinate"
    );
    assert!(result["anchor"]["frame"].get("code_instance_id").is_none());
    assert_eq!(result["coverage"]["total"]["weight"], 23.0);
    assert_eq!(result["coverage"]["shared"]["weight"], 15.0);
    assert_eq!(result["coverage"]["total"]["sample_count"], 4);
    assert_eq!(result["frame_mappings"][0]["evidence"], json!(["unbound"]));
}

#[tokio::test]
async fn ambiguous_frame_targets_or_coordinates_remain_unknown_without_duplicate_contribution() {
    for targets in [
        [("target-a", "0x0"), ("target-b", "0x0")],
        [("instance", "0x1000"), ("instance", "0x1001")],
    ] {
        let (_root, store) = fixture();
        store
            .accept("r", "unbound-input", &[unbound_sample()])
            .unwrap();
        store
            .accept(
                "r",
                "ambiguous-frame-map",
                &[
                    frame_mapping("binding-a", "unbound", targets[0].0, targets[0].1),
                    frame_mapping("binding-b", "unbound", targets[1].0, targets[1].1),
                ],
            )
            .unwrap();
        let ops = Operations::new(store);
        let result = ops
            .call(
                "code.inspect",
                json!({"run_id":"r","observation_id":"unbound"}),
            )
            .await
            .unwrap();
        assert_eq!(result["anchor"]["association_status"], "ambiguous");
        assert!(result["anchor"]["code_instance_id"].is_null());
        assert!(result["anchor"]["code_offset"].is_null());
        assert_eq!(
            result["coverage"]["total"],
            json!({"sample_count":1,"weight":13.0})
        );
        assert_eq!(result["coverage"]["unknown"]["weight"], 13.0);
        assert_eq!(result["frame_mappings"].as_array().unwrap().len(), 2);
        let bounded = ops
            .call(
                "code.inspect",
                json!({"run_id":"r","observation_id":"unbound","limit":1}),
            )
            .await
            .unwrap();
        assert_eq!(bounded["truncated"], true);
        assert_eq!(
            bounded["anchor"]["association_ids"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(bounded["coverage"]["unknown"]["weight"], 13.0);
    }
}

#[test]
fn frame_association_admission_checks_real_frames_inline_identity_ranges_and_comparable_lifetimes()
{
    let (_root, store) = fixture();
    store
        .accept("r", "unbound-input", &[unbound_sample()])
        .unwrap();
    let base = frame_mapping("invalid", "unbound", "instance", "0x1000");
    let mut invalids = Vec::new();
    let mut row = base.clone();
    row["attributes"]["code_frame_mapping"]["frame_index"] = json!(1);
    invalids.push(row);
    let mut row = base.clone();
    row["attributes"]["code_frame_mapping"]["code_offset"] = json!("0x1040");
    invalids.push(row);
    let mut row = base.clone();
    row["to"] = json!("s1");
    invalids.push(row);
    let mut row = base.clone();
    row["from"] = json!("instance");
    invalids.push(row);
    invalids.push(frame_mapping("invalid", "s1", "target-a", "0x0"));
    invalids.push(frame_mapping("invalid", "s1", "instance", "0x1001"));
    for (index, row) in invalids.into_iter().enumerate() {
        assert!(
            store
                .accept("r", &format!("invalid-frame-map-{index}"), &[row])
                .is_err(),
            "case {index}"
        );
    }
    for (id, time, attrs, pid) in [
        (
            "expired",
            40,
            json!({"clock":"unix_epoch","clock_alignment":"direct_epoch"}),
            7,
        ),
        (
            "unknown-clock",
            15,
            json!({"clock":"opaque","clock_alignment":"unknown"}),
            7,
        ),
        (
            "other-process",
            15,
            json!({"clock":"unix_epoch","clock_alignment":"direct_epoch"}),
            8,
        ),
    ] {
        let mut sample = unbound_sample();
        sample["id"] = json!(id);
        sample["timestamp_us"] = json!(time);
        sample["attributes"] = attrs;
        sample["pid"] = json!(pid);
        store
            .accept("r", &format!("sample-{id}"), &[sample])
            .unwrap();
        assert!(
            store
                .accept(
                    "r",
                    &format!("map-{id}"),
                    &[frame_mapping(
                        &format!("map-{id}"),
                        id,
                        "instance",
                        "0x1000"
                    )]
                )
                .is_err(),
            "case {id}"
        );
    }
    // An explicitly static unit is valid without fabricating a runtime lifetime.
    store
        .accept(
            "r",
            "static-frame-map",
            &[frame_mapping(
                "static-binding",
                "unbound",
                "target-a",
                "0x0",
            )],
        )
        .unwrap();
    store
        .accept(
            "r",
            "consistent-inline-map",
            &[frame_mapping("inline-binding", "s1", "instance", "0x1000")],
        )
        .unwrap();
}

#[tokio::test]
async fn missing_origin_keeps_bounded_recording_history_separate_from_anchor_facts() {
    let (_root, store) = fixture();
    let mut run = store.run("r").unwrap();
    run.diagnostics = vec!["λ".repeat(2100), String::new()];
    run.diagnostics.extend(
        (0..25).map(|index| format!("Historical attempt {index}: incomplete compiler export")),
    );
    store.save_run(&run).unwrap();
    let ops = Operations::new(store);
    let missing = ops
        .call("code.inspect", json!({"run_id":"r","observation_id":"s3"}))
        .await
        .unwrap();
    assert!(
        missing["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message
                .as_str()
                .unwrap()
                .contains("No accepted code_maps_to range covers this captured byte coordinate"))
    );
    let history = &missing["recording_diagnostics"];
    assert_eq!(history["messages"].as_array().unwrap().len(), 20);
    assert_eq!(
        history["messages"][0].as_str().unwrap().chars().count(),
        2048
    );
    assert_eq!(history["truncated"], true);
    // A display limit must not make accepted origin evidence appear absent.
    let mapped = ops
        .call(
            "code.inspect",
            json!({"run_id":"r","observation_id":"s1","limit":1}),
        )
        .await
        .unwrap();
    assert!(mapped.get("recording_diagnostics").is_none());
    let operation = hunin::operations::catalog()
        .iter()
        .find(|operation| operation["name"] == "code.inspect")
        .unwrap();
    jsonschema::validator_for(&operation["outputSchema"])
        .unwrap()
        .validate(&missing)
        .unwrap();
}

#[tokio::test]
async fn invalid_frame_or_non_code_observation_never_becomes_an_investigation_anchor() {
    let (_root, store) = fixture();
    store.accept("r","ordinary-event",&[json!({"schema_version":1,"kind":"event","id":"control","run_id":"r","producer_id":"fixture","pid":7,"tid":"main","timestamp_us":15,"basis":"observed","name":"ordinary.control"})]).unwrap();
    let mut run = store.run("r").unwrap();
    run.runtime = "node".into();
    run.command = vec!["must-not-launch".into()];
    store.save_run(&run).unwrap();
    let ops = Operations::new(store);
    for (observation, index) in [
        ("s1", 2),
        ("control", 0),
        ("map-a", 0),
        ("unmapped", 0),
        ("target-a", 1),
        ("instance", 1),
    ] {
        let params = json!({"run_id":"r","observation_id":observation,"frame_index":index});
        let result = ops.call("code.inspect", params.clone()).await.unwrap();
        assert!(
            result["anchor"].is_null(),
            "invalid anchor {observation}/{index}: {result}"
        );
        assert_eq!(result["coverage"]["status"], "unavailable");
        assert!(result["coverage"]["total"]["weight"].is_null());
        let plan = ops.call("code.plan", params).await.unwrap();
        assert!(plan["anchor"].is_null());
        assert!(
            plan["steps"]
                .as_array()
                .unwrap()
                .iter()
                .all(|step| step["status"] == "blocked" && step["operation"].is_null()),
            "invalid anchor plan: {plan}"
        );
    }
    // An explicitly static unit is a legitimate anchor, without an invented frame.
    let unit = ops
        .call(
            "code.inspect",
            json!({"run_id":"r","observation_id":"target-a","frame_index":0}),
        )
        .await
        .unwrap();
    assert_eq!(unit["anchor"]["code_instance_id"], "target-a");
    assert!(unit["anchor"]["frame"].is_null());
    let plan = ops
        .call(
            "code.plan",
            json!({"run_id":"r","observation_id":"target-a","frame_index":0}),
        )
        .await
        .unwrap();
    assert!(!plan["anchor"].is_null());
    assert!(
        plan["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["operation"] == "code.inspect")
    );
}

fn unbound_availability(id: &str, state: &str) -> Value {
    json!({"schema_version":1,"kind":"event","id":id,"run_id":"r","producer_id":"fixture","pid":7,"tid":"main","timestamp_us":15,"basis":"observed","name":"code.mapping_unavailable","attributes":{"code_mapping_unavailable":{"schema_version":1,"anchor":{"observation_id":"unbound","frame_index":0},"state":state,"reason_code":"provider_reported_availability"}}})
}

#[tokio::test]
async fn explicit_unbound_unmapped_state_preserves_unknown_precedence_for_conflicting_evidence() {
    let (_root, store) = fixture();
    store
        .accept(
            "r",
            "unbound-input",
            &[
                unbound_sample(),
                unbound_availability("explicit-unmapped", "unmapped"),
            ],
        )
        .unwrap();
    let ops = Operations::new(store.clone());
    let result = ops
        .call(
            "code.inspect",
            json!({"run_id":"r","observation_id":"unbound"}),
        )
        .await
        .unwrap();
    assert_eq!(
        result["coverage"]["total"],
        json!({"sample_count":1,"weight":13.0})
    );
    assert_eq!(
        result["coverage"]["unmapped"],
        json!({"sample_count":1,"weight":13.0})
    );
    assert_eq!(result["coverage"]["unknown"]["weight"], 0.0);
    store
        .accept(
            "r",
            "ambiguous-unbound-map",
            &[
                frame_mapping("binding-a", "unbound", "target-a", "0x0"),
                frame_mapping("binding-b", "unbound", "target-b", "0x0"),
            ],
        )
        .unwrap();
    let ambiguous = ops
        .call(
            "code.inspect",
            json!({"run_id":"r","observation_id":"unbound"}),
        )
        .await
        .unwrap();
    assert_eq!(ambiguous["anchor"]["association_status"], "ambiguous");
    assert_eq!(ambiguous["coverage"]["unknown"]["weight"], 13.0);
    assert_eq!(ambiguous["coverage"]["unmapped"]["weight"], 0.0);

    let (_root, store) = fixture();
    store
        .accept(
            "r",
            "conflicting-availability",
            &[
                unbound_sample(),
                unbound_availability("explicit-unmapped", "unmapped"),
                unbound_availability("explicit-unknown", "unknown"),
            ],
        )
        .unwrap();
    let unknown = Operations::new(store)
        .call(
            "code.inspect",
            json!({"run_id":"r","observation_id":"unbound"}),
        )
        .await
        .unwrap();
    assert_eq!(unknown["coverage"]["unknown"]["weight"], 13.0);
    assert_eq!(unknown["coverage"]["unmapped"]["weight"], 0.0);
}

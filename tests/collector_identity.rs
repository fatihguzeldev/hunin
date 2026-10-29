use hunin::{
    collection::{CapturePlan, CaptureRequest},
    model::runs::Run,
    operations::Operations,
    storage::Store,
};
use serde_json::json;
use sha2::{Digest, Sha256};

fn recording(id: &str, request: &CaptureRequest, plan: &CapturePlan) -> Run {
    let settings = plan.settings(request);
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&settings).unwrap())
    );
    Run {
        id: id.into(),
        name: id.into(),
        status: "completed".into(),
        command: request.command.clone(),
        cwd: request.cwd.to_string_lossy().into_owned(),
        runtime: request.runtime.clone(),
        started_at_us: 10,
        finished_at_us: Some(30),
        exit_code: Some(0),
        diagnostics: vec![],
        source: json!({
            "fingerprint":"controlled-source",
            "runtime_version":plan.runtime_version,
            "collector_identity":plan.collector_identity,
            "collector_settings":settings,
            "collector_settings_fingerprint":fingerprint,
            "environment_fingerprint":"controlled-environment",
            "workload_inputs":{"fingerprint":"controlled-inputs"},
            "correctness_recipe_fingerprint":"controlled-recipe",
            "supervised_elapsed_us":10,
            "supervised_execution_scope":"target_process",
        }),
        correctness: json!({"passed":true}),
    }
}

#[tokio::test]
async fn measurement_contract_changes_disqualify_otherwise_equivalent_comparisons() {
    for runtime in ["node", "deno"] {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        let request = CaptureRequest {
            command: vec![runtime.into(), "target".into()],
            cwd: data.path().into(),
            artifact_dir: data.path().join("capture"),
            run_id: "capture".into(),
            runtime: runtime.into(),
            env: Default::default(),
        };
        let mut plan: CapturePlan = serde_json::from_value(json!({
            "command":request.command,
            "runtime_version":"controlled-runtime",
            "collector_identity": {
                "kind":format!("{runtime}-cpu-prof"),
                "measurement_contract":format!("{runtime}-v1"),
                "completion_scope":"controlled-profile-delivery",
            },
        }))
        .unwrap();
        let before = recording("before", &request, &plan);
        let after = recording("after", &request, &plan);
        store.save_run(&before).unwrap();
        store.save_run(&after).unwrap();
        let operations = Operations::new(store.clone());
        let compare = json!({"before":"before","after":"after"});
        assert_eq!(
            operations
                .call("comparisons.create", compare.clone())
                .await
                .unwrap()["comparable"],
            true,
        );

        plan.collector_identity.as_mut().unwrap()["measurement_contract"] =
            json!("different-measurement-meaning");
        let changed = recording("after", &request, &plan);
        assert_ne!(
            before.source["collector_settings_fingerprint"],
            changed.source["collector_settings_fingerprint"],
        );
        store.save_run(&changed).unwrap();
        let result = operations
            .call("comparisons.create", compare)
            .await
            .unwrap();
        assert_eq!(result["comparable"], false);
        assert_eq!(result["reasons"], json!(["Collector settings differ"]));
    }
}

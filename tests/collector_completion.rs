use hunin::{execution::Engine, storage::Store};
use serde_json::json;

#[tokio::test]
async fn node_profile_delivery_keeps_unenumerated_producers_unknown() {
    let project = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(data.path()).unwrap();
    let engine = Engine::new(store.clone());
    let run = engine
        .start(
            serde_json::from_value(json!({
                "command":["node", "-e", "console.log('target-ok')"],
                "runtime":"node", "cwd":project.path(), "expected_stdout":"target-ok\n"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let run = engine.wait(&run.id).await.unwrap();
    assert_eq!(run.status, "completed", "{:?}", run.diagnostics);
    assert_eq!(run.source["collection_completion"]["state"], "complete");
    assert_eq!(
        run.source["collection_completion"]["scope"],
        "declared_profile_artifacts"
    );
    assert!(run.source["collection_completion"]["expected_producers"].is_null());
    assert!(run.source["collection_completion"]["observed_producers"].is_null());
    assert!(
        store
            .artifacts(&run.id)
            .unwrap()
            .iter()
            .any(|artifact| artifact.format == "v8-cpuprofile")
    );
}

#[tokio::test]
#[ignore = "requires the selected Deno executable in HUNIN_TEST_DENO"]
async fn incomplete_deno_lifecycle_retains_raw_before_rejecting_collection() {
    let executable = std::env::var("HUNIN_TEST_DENO").expect("set HUNIN_TEST_DENO");
    let project = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let main = project.path().join("target.ts");
    std::fs::write(&main, "console.log('target-ok');").unwrap();
    let store = Store::open(data.path()).unwrap();
    let engine = Engine::new(store.clone());
    let run = engine
        .start(
            serde_json::from_value(json!({
                "command":[executable,"run","--deny-env=HUNIN_DENO_WORKER_SESSION",main],
                "runtime":"DENO", "cwd":project.path(), "expected_stdout":"target-ok\n"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let run = engine.wait(&run.id).await.unwrap();
    assert_eq!(run.runtime, "deno");
    assert_eq!(run.status, "failed");
    assert_eq!(run.correctness["passed"], true);
    assert_eq!(run.source["target_outcome"]["status"], "completed");
    assert_eq!(run.source["collection_status"], "failed");
    assert_eq!(run.source["collection_completion"]["state"], "incomplete");
    let artifacts = store.artifacts(&run.id).unwrap();
    let profile = artifacts
        .iter()
        .find(|artifact| artifact.format == "v8-cpuprofile")
        .expect("raw CPU profile must survive the completion gate");
    assert!(store.artifact_path(&run.id, &profile.id).unwrap().is_file());
    let report = artifacts
        .iter()
        .find(|artifact| artifact.name.starts_with("deno-workers-"))
        .expect("planned lifecycle evidence must survive the completion gate");
    let bytes = std::fs::read_to_string(store.artifact_path(&run.id, &report.id).unwrap()).unwrap();
    assert!(bytes.contains("planned"));
}

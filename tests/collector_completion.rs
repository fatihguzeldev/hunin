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
        run.source["raw_retention"]["name"],
        "capture-raw-retention-v1"
    );
    assert_eq!(run.source["raw_retention"]["complete"], true);
    assert_eq!(run.source["raw_retention"]["inventory_complete"], true);
    assert_eq!(
        run.source["raw_retention"]["source_read_bytes"],
        run.source["raw_retention"]["retained_bytes"]
    );
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

#[tokio::test]
#[ignore = "requires the selected Bun executable in HUNIN_TEST_BUN"]
async fn bun_main_and_earlier_segments_cannot_mask_a_failed_worker_final_save() {
    let executable = std::env::var("HUNIN_TEST_BUN").expect("set HUNIN_TEST_BUN");
    let project = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let main = project.path().join("target.ts");
    std::fs::write(project.path().join("worker.ts"), r#"
        import inspector from 'node:inspector';
        import { readdirSync } from 'node:fs';
        import { threadId } from 'node:worker_threads';
        const deadline = Date.now() + 2000;
        while (!readdirSync(process.env.HUNIN_JS_PROFILE_DIR).some(name => name.startsWith('CPU.' + process.pid + '.' + threadId + '.'))) {
            if (Date.now() > deadline) throw Error('control produced no earlier worker segment');
            await new Promise(resolve => setTimeout(resolve, 5));
        }
        const original = inspector.Session.prototype.post;
        inspector.Session.prototype.post = function(method, ...args) {
            if (method === 'Profiler.stop') {
                args.at(-1)(Error('injected worker final save failure'));
                return;
            }
            return original.call(this, method, ...args);
        };
        postMessage('worker-ok');
    "#).unwrap();
    std::fs::write(&main, "const worker=new Worker(new URL('./worker.ts',import.meta.url),{type:'module'});worker.onmessage=event=>{console.log(event.data);worker.terminate();};").unwrap();
    let store = Store::open(data.path()).unwrap();
    let engine = Engine::new(store.clone());
    let run = engine
        .start(
            serde_json::from_value(json!({
                "command":[executable,main], "runtime":"bun", "cwd":project.path(),
                "expected_stdout":"worker-ok\n", "timeout_seconds":10
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let run = engine.wait(&run.id).await.unwrap();
    assert_eq!(run.correctness["passed"], true, "{:?}", run.diagnostics);
    assert_eq!(run.source["target_outcome"]["status"], "completed");
    assert_eq!(run.source["collection_status"], "failed");
    assert_eq!(run.source["collection_completion"]["state"], "incomplete");
    assert!(
        run.source["collection_completion"]["diagnostics"]
            .to_string()
            .contains("injected worker final save failure")
    );
    assert!(
        store
            .artifacts(&run.id)
            .unwrap()
            .iter()
            .filter(|artifact| artifact.format == "v8-cpuprofile")
            .count()
            >= 2,
        "main and earlier worker profiles must survive the failed final save"
    );
}

#[tokio::test]
#[ignore = "requires the selected Bun executable in HUNIN_TEST_BUN"]
async fn bun_finalizes_both_worker_facades_with_observed_producer_identity() {
    let executable = std::env::var("HUNIN_TEST_BUN").expect("set HUNIN_TEST_BUN");
    for node_worker in [false, true] {
        let project = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let main = project.path().join("target.ts");
        let worker_message = if node_worker {
            "import {parentPort} from 'node:worker_threads';parentPort.postMessage('worker-ok');"
        } else {
            "postMessage('worker-ok');"
        };
        std::fs::write(
            project.path().join("worker.ts"),
            format!("const end=Date.now()+120;while(Date.now()<end)Math.sqrt(42);{worker_message}"),
        )
        .unwrap();
        let parent = if node_worker {
            "import {Worker} from 'node:worker_threads';const worker=new Worker(new URL('./worker.ts',import.meta.url));worker.on('message',value=>{console.log(value);void worker.terminate();});"
        } else {
            "const worker=new Worker(new URL('./worker.ts',import.meta.url),{type:'module'});worker.onmessage=event=>{console.log(event.data);worker.terminate();};"
        };
        std::fs::write(&main, parent).unwrap();
        let engine = Engine::new(Store::open(data.path()).unwrap());
        let run = engine.start(serde_json::from_value(json!({
            "command":[executable,main], "runtime":"bun", "cwd":project.path(), "expected_stdout":"worker-ok\n"
        })).unwrap()).await.unwrap();
        let run = engine.wait(&run.id).await.unwrap();
        assert_eq!(
            run.status, "completed",
            "node_worker={node_worker}: {:?}",
            run.diagnostics
        );
        let completion = &run.source["collection_completion"];
        assert_eq!(completion["state"], "complete");
        assert_eq!(
            completion["expected_producers"].as_array().unwrap().len(),
            2,
            "node_worker={node_worker}: {completion}"
        );
        let observed = completion["observed_producers"].as_array().unwrap();
        assert_eq!(observed.len(), 2);
        assert!(
            observed
                .iter()
                .any(|producer| producer["runtime_id"] == "0")
        );
        assert!(
            observed
                .iter()
                .any(|producer| producer["runtime_id"].as_str().is_some_and(|id| id != "0"))
        );
        assert_eq!(
            run.source["collector_identity"]["measurement_contract"],
            "bun-producers-v1"
        );
    }
}

#[tokio::test]
#[ignore = "requires the selected Bun executable in HUNIN_TEST_BUN"]
async fn bun_observed_child_processes_cannot_substitute_the_planned_root() {
    let executable = std::env::var("HUNIN_TEST_BUN").expect("set HUNIN_TEST_BUN");
    let project = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let main = project.path().join("target.ts");
    std::fs::write(&main, "import{spawnSync}from'node:child_process';const result=spawnSync(process.execPath,['-e',\"console.log('child-ok')\"],{encoding:'utf8'});process.stdout.write(result.stdout);if(result.status!==0)process.exit(2);").unwrap();
    let engine = Engine::new(Store::open(data.path()).unwrap());
    let run = engine.start(serde_json::from_value(json!({
        "command":[executable,main], "runtime":"bun", "cwd":project.path(), "expected_stdout":"child-ok\n"
    })).unwrap()).await.unwrap();
    let run = engine.wait(&run.id).await.unwrap();
    assert_eq!(run.status, "completed", "{:?}", run.diagnostics);
    let observed = run.source["collection_completion"]["observed_producers"]
        .as_array()
        .unwrap();
    assert_eq!(observed.len(), 2);
    assert!(
        observed
            .iter()
            .all(|producer| producer["runtime_id"] == "0")
    );
    assert_ne!(observed[0]["process_id"], observed[1]["process_id"]);
}

use hunin::{model::runs::Run, operations::Operations, storage::Store};
use serde_json::json;

async fn finished(operations: &Operations, run_id: &str) -> serde_json::Value {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let report = operations
                .call("calls.query", json!({"run_id":run_id}))
                .await
                .unwrap();
            if report["status"] != "running" {
                return report;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("counting recording must settle")
}

#[tokio::test]
async fn actual_python_engine_counts_scopes_reuses_requests_and_retains_parent_discovery() {
    let Ok(python) = std::env::var("HUNIN_TEST_COUNTS_PYTHON") else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    parent(&store, "python");
    let mut original = store.run("sampled").unwrap();
    original.command = vec![python, "-c".into(), "def leaf(): return 7\ndef other(): return 11\nfor _ in range(40): leaf()\nfor _ in range(9): other()\nprint('checksum',40*7+9*11)".into()];
    store.save_run(&original).unwrap();
    let operations = Operations::new(store.clone());
    let selected =
        json!({"run_id":"sampled","functions":[{"name":"leaf"}],"request_id":"selected"});
    original.source["correctness_recipe"]["expected_stdout_sha256"] = json!(42);
    store.save_run(&original).unwrap();
    assert!(
        operations
            .call("calls.measure", selected.clone())
            .await
            .is_err()
    );
    original.source["correctness_recipe"]["expected_stdout_sha256"] = serde_json::Value::Null;
    store.save_run(&original).unwrap();
    let result = operations
        .call("calls.measure", selected.clone())
        .await
        .unwrap();
    assert_eq!(result["plan"]["status"], "ready", "{result:#}");
    let id = result["run"]["id"].as_str().unwrap();
    let report = finished(&operations, id).await;
    assert_eq!(report["status"], "complete", "{report:#}");
    assert_eq!(report["requested_scope"], "selected");
    assert_eq!(report["collection_scope"], "all_then_filter");
    assert_eq!(report["counts"].as_array().unwrap().len(), 1);
    assert_eq!(report["counts"][0]["function"]["name"], "leaf");
    assert_eq!(report["counts"][0]["calls"], 40);
    let retry = operations.call("calls.measure", selected).await.unwrap();
    assert_eq!(retry["reused"], true);
    assert_eq!(retry["run"]["id"], id);
    assert!(
        operations
            .call(
                "calls.measure",
                json!({"run_id":"sampled","request_id":"selected"})
            )
            .await
            .is_err()
    );
    assert_eq!(store.runs(100, 0).unwrap().len(), 2);
    let all = operations
        .call("calls.measure", json!({"run_id":id,"request_id":"all"}))
        .await
        .unwrap();
    assert_eq!(all["parent_run_id"], "sampled");
    let report = finished(&operations, all["run"]["id"].as_str().unwrap()).await;
    assert_eq!(report["status"], "complete", "{report:#}");
    let rows = report["counts"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .find(|row| row["function"]["name"] == "leaf")
            .unwrap()["calls"],
        40
    );
    assert_eq!(
        rows.iter()
            .find(|row| row["function"]["name"] == "other")
            .unwrap()["calls"],
        9
    );
    let reopened = operations
        .call("calls.query", json!({"run_id":"sampled"}))
        .await
        .unwrap();
    assert_eq!(reopened["run_id"], all["run"]["id"]);
    assert_eq!(store.runs(100, 0).unwrap().len(), 3);
    operations.shutdown().await.unwrap();
}

#[tokio::test]
async fn actual_python_engine_keeps_abrupt_exit_counts_unknown_and_cancels_owned_work() {
    let Ok(python) = std::env::var("HUNIN_TEST_COUNTS_PYTHON") else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    parent(&store, "python");
    let mut original = store.run("sampled").unwrap();
    original.command = vec![
        python.clone(),
        "-c".into(),
        "import os\ndef leaf(): return 1\nleaf()\nos._exit(0)".into(),
    ];
    store.save_run(&original).unwrap();
    let operations = Operations::new(store.clone());
    let run = operations
        .call(
            "calls.measure",
            json!({"run_id":"sampled","request_id":"abrupt"}),
        )
        .await
        .unwrap();
    let report = finished(&operations, run["run"]["id"].as_str().unwrap()).await;
    assert_eq!(report["status"], "incomplete", "{report:#}");
    assert_eq!(report["counts"], json!([]));
    original.command = vec![
        python,
        "-c".into(),
        "import time\nprint('started',flush=True)\ntime.sleep(20)".into(),
    ];
    store.save_run(&original).unwrap();
    let run = operations
        .call(
            "calls.measure",
            json!({"run_id":"sampled","request_id":"cancel"}),
        )
        .await
        .unwrap();
    let id = run["run"]["id"].as_str().unwrap();
    operations
        .call("runs.cancel", json!({"run_id":id}))
        .await
        .unwrap();
    let report = finished(&operations, id).await;
    assert_eq!(report["status"], "canceled", "{report:#}");
    operations.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn actual_native_engine_preserves_the_workload_and_records_the_counted_build_identity() {
    if std::env::var_os("HUNIN_TEST_COUNTS_NATIVE").is_none() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("main.cpp");
    let binary = root.path().join("original");
    std::fs::write(&source,"#include <cstdio>\nextern \"C\" __attribute__((noinline)) int leaf(int x) { return x+3; }\nint main(){ int sum=0; for(int i=0;i<37;i++) sum+=leaf(i); std::printf(\"%d\\n\",sum); }\n").unwrap();
    let output = std::process::Command::new("clang++")
        .arg("-O2")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let original_bytes = std::fs::read(&binary).unwrap();
    let store = Store::open(&root.path().join("data")).unwrap();
    parent(&store, "cpp");
    let mut original = store.run("sampled").unwrap();
    original.command = vec![binary.to_string_lossy().into_owned()];
    original.cwd = root.path().display().to_string();
    store.save_run(&original).unwrap();
    let operations = Operations::new(store.clone());
    let params = json!({"run_id":"sampled","allow_rebuild":true,"build_command":["clang++","-O2",source,"-o","{output}"],"functions":[{"name":"leaf"}]});
    let result = operations.call("calls.measure", params).await.unwrap();
    assert_eq!(result["plan"]["status"], "ready", "{result:#}");
    let id = result["run"]["id"].as_str().unwrap();
    let report = finished(&operations, id).await;
    assert_eq!(report["status"], "complete", "{report:#}");
    assert_eq!(report["counts"][0]["calls"], 37);
    assert!(report["counts"][0]["resumes"].is_null());
    let child = store.run(id).unwrap();
    assert_eq!(
        child.source["workload_executable"],
        binary.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(
        child.source["executable_sha256"],
        child.source["collector_identity"]["target_sha256"]
    );
    assert_ne!(
        child.source["executable_sha256"],
        child.source["workload_executable_sha256"]
    );
    assert_eq!(child.source["build_source_verified"], false);
    assert_eq!(std::fs::read(binary).unwrap(), original_bytes);
    operations.shutdown().await.unwrap();
}

fn parent(store: &Store, runtime: &str) {
    store.save_run(&Run {
        id: "sampled".into(), name: "original workload".into(), status: "completed".into(),
        command: vec!["node".into(), "app.js".into()], cwd: store.root().display().to_string(),
        runtime: runtime.into(), started_at_us: 1, finished_at_us: Some(2), exit_code: Some(0),
        diagnostics: vec![], source: json!({"correctness_recipe":{"expected_exit_code":0,"expected_stdout_sha256":null},"environment":{"overrides":{}}}),
        correctness: json!({"passed":true}),
    }).unwrap();
}

#[tokio::test]
async fn reading_a_sampled_recording_does_not_invent_counts_or_launch_a_target() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    parent(&store, "node");
    let operations = Operations::new(store.clone());
    let result = operations
        .call("calls.query", json!({"run_id":"sampled"}))
        .await
        .unwrap();
    assert_eq!(result["status"], "not_collected");
    assert_eq!(result["counts"], json!([]));
    assert_eq!(store.runs(100, 0).unwrap().len(), 1);
    operations.shutdown().await.unwrap();
}

#[tokio::test]
async fn unsupported_automatic_counting_returns_a_plan_without_a_hidden_fallback_run() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    parent(&store, "node");
    let operations = Operations::new(store.clone());
    let plan = operations
        .call("calls.plan", json!({"run_id":"sampled"}))
        .await
        .unwrap();
    assert_eq!(plan["plan"]["status"], "blocked");
    let result = operations
        .call("calls.measure", json!({"run_id":"sampled"}))
        .await
        .unwrap();
    assert!(result.get("run").is_none());
    assert_eq!(result["plan"]["status"], "blocked");
    assert_eq!(store.runs(100, 0).unwrap().len(), 1);
    operations.shutdown().await.unwrap();
}

#[tokio::test]
async fn count_requests_validate_scope_and_rebuild_intent_before_launch() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    parent(&store, "node");
    let operations = Operations::new(store.clone());
    for invalid in [
        json!({"run_id":"sampled","max_functions":0}),
        json!({"run_id":"sampled","functions":[{"name":"parse","line":0}]}),
        json!({"run_id":"sampled","build_command":["clang","main.c","-o","{output}"]}),
    ] {
        assert!(operations.call("calls.measure", invalid).await.is_err());
    }
    assert_eq!(store.runs(100, 0).unwrap().len(), 1);
    operations.shutdown().await.unwrap();
}

#[tokio::test]
async fn reopening_a_parent_reads_its_retained_child_without_relaunching_and_preserves_observed_zero()
 {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    parent(&store, "node");
    let mut child = store.run("sampled").unwrap();
    child.id = "counted".into();
    child.source = json!({"collector_identity":{"name":"fixture"},"call_counting":{"parent_run_id":"sampled","request_key":"key","requested_scope":"selected"},"collection_status":"completed"});
    store.save_run(&child).unwrap();
    // An admitted child survives even when its reservation acknowledgement was never saved.
    let attrs = json!({"clock":"unix_epoch","clock_alignment":"direct_epoch","method":"fixture","collection_scope":"all_then_filter","count_semantics":"starts","counter_temporality":"delta","function":{"name":"parse","file":"parser.py","line":1,"code_id":"fixture:instance"}});
    store.accept("counted","counts",&[
        json!({"schema_version":1,"kind":"metric","id":"count","run_id":"counted","producer_id":"calls:counted","pid":1,"tid":"process","timestamp_us":100,"basis":"observed","name":"function.calls","value":0.0,"unit":"count","scope":"process","attributes":attrs}),
        json!({"schema_version":1,"kind":"event","id":"complete","run_id":"counted","producer_id":"calls:counted","pid":1,"tid":"process","timestamp_us":100,"basis":"observed","name":"function.counts.complete","attributes":{"status":"complete","clock":"unix_epoch","clock_alignment":"direct_epoch","method":"fixture","collection_scope":"all_then_filter"}})
    ]).unwrap();
    let operations = Operations::new(store.clone());
    let result = operations
        .call("calls.query", json!({"run_id":"sampled"}))
        .await
        .unwrap();
    assert_eq!(result["run_id"], "counted");
    assert_eq!(result["parent_run_id"], "sampled");
    assert_eq!(result["status"], "complete");
    assert_eq!(result["counts"][0]["calls"], 0);
    assert_eq!(
        result["counts"][0]["function"]["code_id"],
        "fixture:instance"
    );
    assert!(result["counts"][0]["resumes"].is_null());
    assert_eq!(store.runs(100, 0).unwrap().len(), 2);
    child.correctness["passed"] = serde_json::Value::Null;
    store.save_run(&child).unwrap();
    assert_ne!(
        operations
            .call("calls.query", json!({"run_id":"sampled"}))
            .await
            .unwrap()["status"],
        "complete"
    );
    operations.shutdown().await.unwrap();
}

#[tokio::test]
async fn arbitrary_accepted_metrics_cannot_masquerade_as_function_invocations() {
    for (pointer, contradictory) in [
        ("/unit", json!("bytes")),
        ("/scope", json!("thread")),
        ("/basis", json!("explicit")),
        ("/attributes/count_semantics", json!("resumes")),
        ("/attributes/counter_temporality", json!("cumulative")),
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        parent(&store, "node");
        let mut child = store.run("sampled").unwrap();
        child.id = "counted".into();
        child.source = json!({"collector_identity":{"name":"fixture"},"call_counting":{"parent_run_id":"sampled","requested_scope":"all"},"collection_status":"completed"});
        store.save_run(&child).unwrap();
        let mut metric = json!({"schema_version":1,"kind":"metric","id":"count","run_id":"counted","producer_id":"calls:counted","pid":1,"tid":"process","timestamp_us":100,"basis":"observed","name":"function.calls","value":40,"unit":"count","scope":"process","attributes":{"clock":"unix_epoch","clock_alignment":"direct_epoch","method":"fixture","collection_scope":"all_then_filter","counter_temporality":"delta","count_semantics":"starts","function":{"name":"parse","file":null,"line":null}}});
        *metric.pointer_mut(pointer).unwrap() = contradictory;
        store
            .accept("counted", "count-control", &[metric.clone()])
            .unwrap();
        let operations = Operations::new(store.clone());
        let result = operations
            .call("calls.query", json!({"run_id":"counted"}))
            .await;
        assert!(
            result.is_err(),
            "{pointer} must not be presented as an invocation count"
        );
        assert_eq!(store.runs(100, 0).unwrap().len(), 2);
        assert_eq!(
            store.records("counted").unwrap()[0],
            metric
        );
        operations.shutdown().await.unwrap();
    }
}

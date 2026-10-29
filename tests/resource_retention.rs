use hunin::{execution::Engine, storage::Store};
use serde_json::json;

#[tokio::test]
async fn actual_resource_delivery_and_retention_reports_keep_target_and_metric_semantics() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("store")).unwrap();
    let engine = Engine::new(store.clone());
    let run=engine.start(serde_json::from_value(json!({
        "command":[if cfg!(windows){"python"}else{"python3"},"-c","import sys,time; time.sleep(0.35); sys.stdout.buffer.write(b'resource-ok\\n')"],
        "runtime":"none","cwd":root.path(),"expected_stdout":"resource-ok\n",
    })).unwrap()).await.unwrap();
    let run = engine.wait(&run.id).await.unwrap();
    assert_eq!(run.status, "completed", "{:?}", run.diagnostics);
    assert_eq!(run.correctness["passed"], true);
    let observation = &run.source["resource_observation"];
    let delivery = &observation["queue_delivery"];
    assert_eq!(delivery["queue_capacity_batches"], 4);
    assert_eq!(delivery["os_observation_gaps"], "unknown");
    let count = |name: &str| delivery[name].as_u64().unwrap();
    assert_eq!(
        count("generated_rows"),
        count("queued_rows")
            + count("queue_full_dropped_rows")
            + count("receiver_closed_dropped_rows")
            + count("stop_discarded_rows")
    );
    let retention = &observation["retention"];
    assert_eq!(retention["name"], "resource-retention-v1");
    assert_eq!(retention["limits"]["rows"], 100_000);
    assert_eq!(retention["limits"]["serialized_bytes"], 32 * 1024 * 1024);
    assert_eq!(retention["received_rows"], delivery["queued_rows"]);
    assert_eq!(retention["truncated"], false);
    assert_eq!(retention["discarded_rows"], 0);
    let rows = store.records(&run.id).unwrap();
    let rows = rows
        .iter()
        .filter(|row| row["kind"] == "metric")
        .collect::<Vec<_>>();
    assert!(!rows.is_empty());
    assert_eq!(
        rows.len() as u64,
        retention["retained_rows"].as_u64().unwrap()
    );
    assert!(rows.iter().all(|row| row["basis"] == "observed"
        && row["scope"] == "process"
        && (row["name"] == "process.rss"
            && row["unit"] == "bytes"
            && row["attributes"]["temporality"] == "gauge"
            || row["name"] == "process.cpu_time"
                && row["unit"] == "milliseconds"
                && row["attributes"]["temporality"] == "cumulative")));
    engine.shutdown().await.unwrap();
}

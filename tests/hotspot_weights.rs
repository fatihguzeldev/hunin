use hunin::{model::runs::Run, operations::Operations, storage::Store};
use serde_json::json;

#[tokio::test]
async fn an_overflowed_hotspot_is_not_reset_by_the_next_sample_or_lost_below_the_rank_limit() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let run: Run = serde_json::from_value(json!({"id":"overflow","name":"weights","status":"completed","command":[],"cwd":"/fixture","runtime":"fixture","started_at_us":0,"finished_at_us":1,"exit_code":0,"diagnostics":[],"source":{},"correctness":{}})).unwrap();
    store.save_run(&run).unwrap();
    let mut rows = (0..3).map(|index|json!({"schema_version":1,"kind":"sample","id":format!("large-{index}"),"run_id":"overflow","producer_id":"profiler","pid":1,"tid":"main","timestamp_us":index,"basis":"sampled","weight":1e308,"weight_unit":"samples","stack":[{"function":"large"}]})).collect::<Vec<_>>();
    rows.push(json!({"schema_version":1,"kind":"sample","id":"small","run_id":"overflow","producer_id":"profiler","pid":1,"tid":"main","timestamp_us":4,"basis":"sampled","weight":42,"weight_unit":"samples","stack":[{"function":"small"}]}));
    store.accept("overflow", "profile", &rows).unwrap();
    let result = Operations::new(store)
        .call("profiles.hotspots", json!({"run_id":"overflow","limit":1}))
        .await
        .unwrap();
    assert!(result["total_weight"].is_null());
    assert_eq!(result["hotspots"][0]["function"], "large");
    assert!(result["hotspots"][0]["self_weight"].is_null());
    assert!(result["hotspots"][0]["inclusive_weight"].is_null());
}

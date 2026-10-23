use hunin::query;
use serde_json::json;

#[test]
fn inclusive_stack_weights_do_not_duplicate_total_or_merge_different_units() {
    let records = vec![
        json!({"kind":"sample","stack":[{"function":"root"},{"function":"hot"}],"weight":3,"weight_unit":"samples"}),
        json!({"kind":"sample","stack":[{"function":"root"},{"function":"cool"}],"weight":1,"weight_unit":"samples"}),
        json!({"kind":"sample","stack":[{"function":"hot"}],"weight":900,"weight_unit":"bytes"}),
    ];
    let result = query::hotspots(&records, "samples", 20);
    assert_eq!(result["total_weight"], 4.0);
    let hot = result["hotspots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["function"] == "hot")
        .unwrap();
    assert_eq!(hot["self_weight"], 3.0);
    assert_eq!(hot["inclusive_weight"], 3.0);
    let root = result["hotspots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["function"] == "root")
        .unwrap();
    assert_eq!(root["inclusive_weight"], 4.0);
    assert_eq!(root["self_weight"], 0.0);
}

#[test]
fn unaligned_cpu_clocks_do_not_become_wall_timeline_positions() {
    let records = vec![
        json!({"kind":"sample","timestamp_us":20,"attributes":{"clock_alignment":"unmapped"}}),
        json!({"kind":"span","timestamp_us":100,"duration_us":10,"pid":1}),
    ];
    let result = query::timeline(&records, &json!({}));
    assert_eq!(result["records"].as_array().unwrap().len(), 1);
    assert_eq!(result["bounds"]["start_us"], 100);
    assert_eq!(result["unaligned_count"], 1);
}

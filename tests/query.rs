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

#[test]
fn function_totals_merge_sample_locations_but_keep_build_identity() {
    let records = vec![
        json!({"kind":"sample","stack":[{"function":"hot","file":"work.c","module":"work","code_id":"a","line":4,"address":"0x10"}],"weight":2,"weight_unit":"samples"}),
        json!({"kind":"sample","stack":[{"function":"hot","file":"work.c","module":"work","code_id":"a","line":5,"address":"0x20"}],"weight":3,"weight_unit":"samples"}),
        json!({"kind":"sample","stack":[{"function":"hot","file":"work.c","module":"work","code_id":"b","line":4,"address":"0x10"}],"weight":7,"weight_unit":"samples"}),
    ];
    let result = query::hotspots(&records, "samples", 20);
    assert_eq!(result["hotspots"].as_array().unwrap().len(), 2);
    let old = result["hotspots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["code_id"] == "a")
        .unwrap();
    assert_eq!(old["self_weight"], 5.0);
    assert_eq!(
        query::tree(&records, "samples")["roots"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn unmapped_view_ignores_epoch_window_and_all_view_keeps_separate_arrays() {
    let records = vec![
        json!({"kind":"event","timestamp_us":20,"pid":1,"attributes":{"clock":"process_cpu","clock_alignment":"unmapped"}}),
        json!({"kind":"span","timestamp_us":100,"duration_us":10,"pid":1}),
    ];
    let unmapped = query::timeline(
        &records,
        &json!({"alignment":"unmapped","start_us":90,"end_us":120}),
    );
    assert_eq!(unmapped["records"].as_array().unwrap().len(), 1);
    assert_eq!(unmapped["records"][0]["timestamp_us"], 20);
    assert!(unmapped["bounds"].is_null());
    let all = query::timeline(&records, &json!({"alignment":"all"}));
    assert_eq!(all["records"].as_array().unwrap().len(), 1);
    assert_eq!(all["unaligned_records"].as_array().unwrap().len(), 1);
    assert_eq!(all["bounds"]["start_us"], 100);
}

#[test]
fn resource_series_keep_scope_thread_namespaces_lifetimes_and_clocks() {
    let mut records = vec![];
    for (id, context, tid, producer, start, clock) in [
        ("a", "one", "1", "sdk", 1, "epoch"),
        ("b", "two", "1", "sdk", 1, "epoch"),
        ("c", "one", "1", "sdk", 2, "epoch"),
        ("d", "one", "1", "native", 1, "epoch"),
        ("e", "one", "1", "sdk", 1, "cpu"),
    ] {
        let attrs = if clock == "cpu" {
            json!({"clock":"process_cpu","clock_alignment":"unmapped","process_started_at_seconds":start})
        } else {
            json!({"process_started_at_seconds":start})
        };
        records.push(json!({"id":id,"kind":"metric","name":"allocated","value":1,"unit":"bytes","scope":"context","pid":4,"tid":tid,"producer_id":producer,"context_id":context,"timestamp_us":100,"attributes":attrs}));
    }
    let result = query::resources_page(&records, &json!({"point_limit":100}));
    assert_eq!(result["series"].as_array().unwrap().len(), 5);
    assert!(
        result["series"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["context_id"].is_string()
                && s["producer_id"].is_string()
                && s["tid"].is_string())
    );
    assert_eq!(
        result["series"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["clock_alignment"] == "unmapped")
            .count(),
        1
    );
}

#[test]
fn resource_points_page_within_series_and_apply_epoch_window() {
    let records=(0..6).map(|i|json!({"id":format!("point-{i}"),"kind":"metric","name":"rss","value":i,"unit":"bytes","scope":"process","pid":1,"tid":"process","producer_id":"lifetime","timestamp_us":100+i})).collect::<Vec<_>>();
    let first = query::resources_page(
        &records,
        &json!({"point_limit":2,"start_us":101,"end_us":104}),
    );
    assert_eq!(first["returned_points"], 2);
    assert_eq!(first["total_points"], 4);
    assert_eq!(first["next_cursor"], "2");
    assert_eq!(first["series"][0]["points"][0]["timestamp_us"], 101);
    assert_eq!(first["series"][0]["truncated"], true);
    let second = query::resources_page(
        &records,
        &json!({"point_limit":2,"start_us":101,"end_us":104,"cursor":"2"}),
    );
    assert!(second["next_cursor"].is_null());
    assert_eq!(second["series"][0]["points"][0]["timestamp_us"], 103);
    assert_eq!(second["series"][0]["clock_alignment"], "aligned");
}

#[test]
fn bounded_trees_expose_navigation_to_omitted_children_and_root_siblings() {
    let records = vec![
        json!({"kind":"sample","weight":3,"weight_unit":"samples","stack":[{"function":"root"},{"function":"hot"},{"function":"leaf"}]}),
        json!({"kind":"sample","weight":1,"weight_unit":"samples","stack":[{"function":"other"}]}),
    ];
    let first = query::tree_page(&records, &json!({"limit":1}));
    assert_eq!(first["returned_nodes"], 1);
    assert_eq!(first["roots"][0]["name"], "root");
    assert_eq!(first["roots"][0]["children_truncated"], true);
    assert_eq!(first["next_cursor"], "1");
    let second = query::tree_page(&records, &json!({"limit":1,"cursor":"1"}));
    assert_eq!(second["roots"][0]["name"], "other");
    let focused = query::tree_page(
        &records,
        &json!({"limit":1,"path":[first["roots"][0]["id"]]}),
    );
    assert_eq!(focused["roots"][0]["name"], "hot");
    assert_eq!(focused["total_weight"], 3.0);
    assert_eq!(focused["roots"][0]["children_truncated"], true);
}

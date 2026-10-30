use super::*;

#[test]
fn summary_entrypoints_preserve_literal_counts_units_weights_and_initial_zero() {
    let records = [
        json!({"kind":"sample","weight_unit":"samples","weight":1}),
        json!({"kind":"span"}),
        json!({"kind":"sample","weight_unit":"samples","weight":3}),
        json!({"kind":"sample","weight_unit":"bytes","weight":5}),
        json!({"kind":"metric"}),
        json!({"kind":"relation"}),
        json!({"kind":"event"}),
    ];
    let expected = json!({"sample_count":3,"total_weight":4.0,"weight_unit":"samples","span_count":1,"metric_count":1,"relation_count":1,"diagnostics":["Multiple sample units exist; query each unit separately."]});
    assert_eq!(summary(&records), expected);
    let mut streamed = Summary::default();
    let mut work = WorkBudget::unlimited();
    for record in &records {
        streamed.add(&record.to_string(), &mut work).unwrap();
    }
    assert_eq!(streamed.finish(), expected);
    assert_eq!(summary(&[])["total_weight"].to_string(), "-0.0");
    assert_eq!(
        Summary::default().finish()["total_weight"].to_string(),
        "0.0"
    );
}

use super::*;
use std::fs;

fn rejected(result: Result<ImportedProfile>) -> anyhow::Error {
    match result {
        Ok(_) => panic!("normalization output budget must reject without returning a prefix"),
        Err(error) => error,
    }
}

#[test]
fn two_profiles_share_one_output_row_budget_without_returning_a_second_prefix() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("raw.jsonl");
    fs::write(&path, b"{\"run_id\":\"run\"}\n").unwrap();
    let mut budget = OutputBudget::with_limits(output::Limits {
        rows: 1,
        ..Default::default()
    });
    assert_eq!(
        import_with_budget_controlled(&path, "jsonl", "run", &mut budget, None)
            .unwrap()
            .records
            .len(),
        1
    );
    let error = rejected(import_with_budget_controlled(
        &path,
        "jsonl",
        "run",
        &mut budget,
        None,
    ));
    assert_eq!(
        error
            .downcast_ref::<NormalizationOutputLimit>()
            .unwrap()
            .dimension,
        OutputDimension::Rows
    );
    assert!(path.is_file());
}

#[test]
fn shared_v8_symbol_cannot_expand_past_a_tiny_output_record_budget() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("raw.cpuprofile");
    fs::write(&path, serde_json::to_vec(&json!({"startTime":0,"nodes":[{"id":1,"callFrame":{"functionName":"long shared symbol".repeat(32)}}],"samples":vec![1;16],"timeDeltas":vec![1;16]})).unwrap()).unwrap();
    let mut budget = OutputBudget::with_limits(output::Limits {
        record_bytes: 256,
        ..Default::default()
    });
    let error = rejected(import_with_budget_controlled(
        &path,
        "v8-cpuprofile",
        "run",
        &mut budget,
        None,
    ));
    assert_eq!(
        error
            .downcast_ref::<NormalizationOutputLimit>()
            .unwrap()
            .dimension,
        OutputDimension::RecordBytes
    );
    assert!(path.is_file());
}

#[test]
fn native_lifecycle_and_reserved_relations_share_the_exact_same_output_budget() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cpu.json");
    fs::write(&path, include_bytes!("fixtures/native-lifetimes/cpu.json")).unwrap();
    fs::write(
        path.with_extension("syms.json"),
        include_bytes!("fixtures/native-lifetimes/cpu.syms.json"),
    )
    .unwrap();
    fs::write(
        path.with_extension("lifetimes.json"),
        include_bytes!("fixtures/native-lifetimes/cpu.lifetimes.json.gz"),
    )
    .unwrap();
    let baseline = import(&path, "samply", "run").unwrap();
    assert!(baseline.records.iter().any(|row| row["kind"] == "relation"));
    assert!(
        baseline
            .records
            .iter()
            .any(|row| row["name"] == "module.observed_lifetime")
    );
    let rows = baseline.records.len() as u64;
    let mut exact = OutputBudget::with_limits(output::Limits {
        rows,
        ..Default::default()
    });
    let exact = import_with_budget_controlled(&path, "samply", "run", &mut exact, None).unwrap();
    assert_eq!(exact.records, baseline.records);
    assert_eq!(exact.input_fingerprint, baseline.input_fingerprint);
    let mut limited = OutputBudget::with_limits(output::Limits {
        rows: rows - 1,
        ..Default::default()
    });
    let error = rejected(import_with_budget_controlled(
        &path,
        "samply",
        "run",
        &mut limited,
        None,
    ));
    let error = error.downcast_ref::<NormalizationOutputLimit>().unwrap();
    assert_eq!(error.dimension, OutputDimension::Rows);
    assert_eq!(error.observed, rows);
    assert!(path.with_extension("lifetimes.json").is_file());
}

#[test]
fn output_refusal_preserves_raw_and_the_previous_selected_result() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::storage::Store::open(&root.path().join("store")).unwrap();
    store.save_run(&serde_json::from_value(json!({"id":"run","name":"output budget","status":"completed","command":[],"cwd":".","runtime":"imported","started_at_us":1,"finished_at_us":2,"exit_code":0,"diagnostics":[],"source":{},"correctness":{}})).unwrap()).unwrap();
    let path = root.path().join("events.jsonl");
    let row = |id| json!({"schema_version":1,"kind":"event","id":id,"run_id":"run","producer_id":"fixture","pid":1,"tid":"main","timestamp_us":1,"basis":"observed","name":"event"});
    fs::write(&path, format!("{}\n", row("old"))).unwrap();
    crate::recordings::publish(&store, "run", import(&path, "jsonl", "run").unwrap()).unwrap();
    let before = store.records_snapshot("run").unwrap();
    let rejected_raw = format!("{}\n{}\n", row("new-a"), row("new-b"));
    fs::write(&path, &rejected_raw).unwrap();
    let artifact = store.add_artifact("run", &path, "jsonl").unwrap();
    let mut budget = OutputBudget::with_limits(output::Limits {
        rows: 1,
        ..Default::default()
    });
    assert!(import_with_budget_controlled(&path, "jsonl", "run", &mut budget, None).is_err());
    let after = store.records_snapshot("run").unwrap();
    assert_eq!(before.result_identity, after.result_identity);
    assert_eq!(before.records, after.records);
    assert_eq!(
        fs::read(store.artifact_path("run", &artifact.id).unwrap()).unwrap(),
        rejected_raw.as_bytes()
    );
    assert_eq!(fs::read(path).unwrap(), rejected_raw.as_bytes());
}

#[test]
fn common_profile_gate_rejects_auxiliary_metadata_even_when_record_output_fits() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("raw.jsonl");
    fs::write(&path, b"{\"run_id\":\"run\"}\n").unwrap();
    let mut budget = OutputBudget::with_limits(output::Limits {
        attachment_bytes: 1,
        ..Default::default()
    });
    let error = rejected(import_with_budget_controlled(
        &path,
        "jsonl",
        "run",
        &mut budget,
        None,
    ));
    let error = error.downcast_ref::<NormalizationOutputLimit>().unwrap();
    assert_eq!(error.dimension, OutputDimension::AttachmentBytes);
    assert_eq!(error.maximum, 1);
    assert_eq!(error.observed, 2);
    assert!(path.is_file());
}

use hunin::{model::runs::Run, operations::Operations, storage::Store};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Command, Output},
};

fn call(data: &Path, operation: &str, params: &str, machine: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hunin"));
    command.arg("--data-dir").arg(data).arg("call");
    if machine {
        command.arg("--json-errors");
    }
    command.arg(operation).arg(params).output().unwrap()
}

fn machine_error(output: &Output) -> Value {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let value: Value = serde_json::from_slice(&output.stderr).unwrap_or_else(|error| {
        panic!(
            "not one shared JSON error: {error}: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(String::from_utf8_lossy(&output.stderr).lines().count(), 1);
    let schema: Value =
        serde_json::from_str(include_str!("../protocol/response.schema.json")).unwrap();
    jsonschema::validator_for(&schema["$defs"]["OperationFailure"])
        .unwrap()
        .validate(&value)
        .unwrap();
    value
}

#[tokio::test]
async fn cli_machine_errors_share_unknown_and_invalid_parameter_origins_with_mcp() {
    let root = tempfile::tempdir().unwrap();
    let operations = Operations::new(Store::open(root.path()).unwrap());
    for (name, params, code) in [
        ("missing.operation", json!({}), "unknown_operation"),
        ("runs.get", json!({}), "invalid_parameters"),
        (
            "runs.get",
            json!({"run_id":"unknown_operation invalid_parameters conflict"}),
            "operation_failed",
        ),
    ] {
        let actual = machine_error(&call(root.path(), name, &params.to_string(), true));
        assert_eq!(actual["code"], code);
        assert_eq!(actual["operation"], name);
        let response=hunin::interfaces::mcp::handle(&operations,&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":params}}),true).await;
        assert_eq!(actual, response["result"]["_meta"]["hunin/operation-error"]);
    }
    let syntax = machine_error(&call(root.path(), "runs.get", "{invalid", true));
    assert_eq!(syntax["code"], "invalid_parameters");
    assert_eq!(syntax["operation"], "runs.get");
    assert!(
        syntax["message"]
            .as_str()
            .unwrap()
            .contains("parameters must be a JSON object")
    );
}

#[tokio::test]
async fn actual_large_legal_record_retains_the_shared_budget_dimension_in_cli_stderr() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store
        .save_run(&Run {
            id: "large".into(),
            name: "synthetic CLI budget".into(),
            status: "completed".into(),
            command: vec![],
            cwd: ".".into(),
            runtime: "imported".into(),
            started_at_us: 1,
            finished_at_us: Some(1),
            exit_code: None,
            diagnostics: vec![],
            source: Value::Null,
            correctness: Value::Null,
        })
        .unwrap();
    store.accept("large","input",&[json!({"schema_version":1,"kind":"event","id":"large-event","run_id":"large","producer_id":"synthetic","pid":1,"tid":"","timestamp_us":1,"basis":"explicit","name":"synthetic","attributes":{"payload":"x".repeat(9*1024*1024)}})]).unwrap();
    let actual = machine_error(&call(
        root.path(),
        "runs.get",
        r#"{"run_id":"large"}"#,
        true,
    ));
    assert_eq!(actual["code"], "work_budget_exceeded");
    assert_eq!(actual["budget"]["dimension"], "record_bytes");
    assert_eq!(actual["budget"]["limit"], 8 * 1024 * 1024);
    let operations = Operations::new(store);
    let response=hunin::interfaces::mcp::handle(&operations,&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"runs.get","arguments":{"run_id":"large"}}}),true).await;
    assert_eq!(actual, response["result"]["_meta"]["hunin/operation-error"]);
}

#[test]
fn default_human_failure_and_success_output_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let human = call(root.path(), "missing.operation", "{}", false);
    assert!(!human.status.success());
    assert!(human.stdout.is_empty());
    assert!(String::from_utf8_lossy(&human.stderr).starts_with("Error: unknown operation"));
    assert!(serde_json::from_slice::<Value>(&human.stderr).is_err());
    let human_success = call(root.path(), "runs.list", "{}", false);
    let machine_success = call(root.path(), "runs.list", "{}", true);
    assert!(human_success.status.success());
    assert!(machine_success.status.success());
    assert!(human_success.stderr.is_empty());
    assert!(machine_success.stderr.is_empty());
    assert_eq!(human_success.stdout, machine_success.stdout);
    assert_eq!(
        serde_json::from_slice::<Value>(&machine_success.stdout).unwrap(),
        json!({"runs":[],"next_cursor":null})
    );
}

#[test]
fn corrupt_retained_manifest_uses_one_machine_error_without_silencing_default_startup_diagnostics()
{
    let root = tempfile::tempdir().unwrap();
    let _store = Store::open(root.path()).unwrap();
    let db = rusqlite::Connection::open(root.path().join("hunin.sqlite")).unwrap();
    db.execute(
        "INSERT INTO runs(id,started_at_us,manifest) VALUES('corrupt',1,'not JSON')",
        [],
    )
    .unwrap();
    let machine = machine_error(&call(root.path(), "missing.operation", "{}", true));
    assert_eq!(machine["code"], "operation_failed");
    assert_eq!(machine["operation"], "missing.operation");
    assert!(
        machine["message"]
            .as_str()
            .unwrap()
            .contains("recording owner recovery failed")
    );
    let human = call(root.path(), "missing.operation", "{}", false);
    assert!(!human.status.success());
    assert!(human.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&human.stderr);
    assert!(stderr.contains("recording owner recovery failed"));
    assert!(stderr.contains("Error: unknown operation"));
    assert_eq!(
        db.query_row("SELECT manifest FROM runs WHERE id='corrupt'", [], |row| {
            row.get::<_, String>(0)
        })
        .unwrap(),
        "not JSON"
    );
}

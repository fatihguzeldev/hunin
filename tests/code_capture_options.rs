use hunin::{
    collection::{CapturePlan, CaptureRequest, prepare},
    execution::StartRequest,
};
use serde_json::json;

#[test]
fn explicit_code_origin_mode_survives_the_execution_request_boundary() {
    let request = serde_json::from_value::<StartRequest>(json!({
        "command": ["node", "--", "work.js"], "runtime": "node",
        "capture_mode": "code-origin",
        "code_options": {"compiler_export": true, "function_filter": "guest.*", "sampling_interval_us": 500}
    }));
    assert!(
        request.is_ok(),
        "the explicit code-origin request must be accepted: {request:?}"
    );
}

#[test]
fn code_origin_requires_the_selected_node_route_before_launch_planning() {
    let root = tempfile::tempdir().unwrap();
    let request: CaptureRequest = serde_json::from_value(json!({
        "command": ["/missing/runtime", "target.py"], "runtime": "cpython", "cwd": root.path(),
        "artifact_dir": root.path().join("capture"), "run_id": "code-mode",
        "capture_mode": "code-origin", "code_options": {}
    }))
    .unwrap();
    let error = prepare(&request).unwrap_err();
    assert!(
        error.to_string().contains("code-origin requires Node"),
        "{error:#}"
    );
}

#[test]
fn code_capture_policy_changes_remain_part_of_comparison_settings() {
    let root = tempfile::tempdir().unwrap();
    let request = |budget| {
        serde_json::from_value::<CaptureRequest>(json!({
            "command": ["node", "work.js"], "runtime": "node", "cwd": root.path(),
            "artifact_dir": root.path().join("capture"), "run_id": "code-mode",
            "capture_mode": "code-origin", "code_options": {"max_artifact_bytes": budget}
        }))
        .unwrap()
    };
    let plan: CapturePlan = serde_json::from_value(json!({"command":["node", "work.js"]})).unwrap();
    assert_ne!(plan.settings(&request(1024)), plan.settings(&request(2048)));
    let ordinary: CaptureRequest = serde_json::from_value(json!({
        "command":["node","work.js"],"runtime":"node","cwd":root.path(),
        "artifact_dir":root.path().join("capture"),"run_id":"ordinary"
    }))
    .unwrap();
    assert!(
        plan.settings(&ordinary).get("capture_mode").is_none(),
        "ordinary existing comparison settings remain stable"
    );
}

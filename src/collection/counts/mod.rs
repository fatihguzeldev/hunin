//! Optional bounded call collectors, separate from ordinary CPU sampling.
mod compiler;
mod completion;
mod python;

use super::{CapturePlan, CaptureRequest, select_runtime};
use crate::model::calls::{CountPlan, CountRequest};
use anyhow::{Result, bail};
use serde_json::json;

pub(crate) fn plan(request: &CaptureRequest, options: &CountRequest) -> Result<CountPlan> {
    options.validate()?;
    let requested_scope = if options.functions.is_empty() {
        "all"
    } else {
        "selected"
    };
    let mut result = CountPlan {
        status: "blocked".into(),
        method: None,
        reason: None,
        requested_scope: requested_scope.into(),
        collection_scope: "all_then_filter".into(),
        functions: options.functions.clone(),
        requires_rebuild: false,
        limitations: vec![],
    };
    if request.command.is_empty() {
        result.reason = Some("a target command is required".into());
        return Ok(result);
    }
    let runtime = select_runtime(&request.runtime).map(|value| value.id);
    let prerequisite = match runtime.as_deref() {
        Ok("cpython" | "python-cpu") => {
            result.method = Some("cpython-monitoring-native".into());
            result.limitations = python::limitations();
            python::prerequisite(request).map(|_| ())
        }
        Ok("c" | "cpp" | "native") => {
            result.method = Some("compiler-hooks".into());
            result.requires_rebuild = true;
            result.limitations = compiler::limitations();
            compiler::prerequisite(request, options).map(|_| ())
        }
        _ => Err(anyhow::anyhow!(
            "No verified automatic function counter is available for runtime {}; CPU sampling remains a separate mode",
            request.runtime
        )),
    };
    match prerequisite {
        Ok(()) => result.status = "ready".into(),
        Err(error) => result.reason = Some(format!("{error:#}")),
    }
    Ok(result)
}

pub(crate) fn prepare(request: &CaptureRequest, options: &CountRequest) -> Result<CapturePlan> {
    let selected = plan(request, options)?;
    if selected.status != "ready" {
        bail!(
            "{}",
            selected
                .reason
                .unwrap_or_else(|| "function counting unavailable".into())
        );
    }
    std::fs::create_dir_all(&request.artifact_dir)?;
    let mut capture = match selected.method.as_deref() {
        Some("cpython-monitoring-native") => python::prepare(request, options)?,
        Some("compiler-hooks") => compiler::prepare(request, options)?,
        _ => bail!("no supported function counter selected"),
    };
    if let Some(identity) = capture.collector_identity.as_mut() {
        identity["count_request"] = json!(options);
        identity["count_plan"] = json!(selected);
    }
    Ok(capture)
}

pub(super) use completion::validate;

#[cfg(test)]
mod tests {
    use super::*;

    fn request(root: &std::path::Path, runtime: &str) -> CaptureRequest {
        CaptureRequest {
            command: vec!["target".into()],
            cwd: root.into(),
            artifact_dir: root.join("capture"),
            run_id: "count-control".into(),
            runtime: runtime.into(),
            capture_mode: Default::default(),
            code_options: None,
            env: Default::default(),
        }
    }

    #[test]
    fn unsupported_routes_and_missing_rebuild_authority_remain_explicit_without_files() {
        let root = tempfile::tempdir().unwrap();
        for runtime in ["rust", "go", "pypy", "graalpy", "node", "c", "cpp"] {
            let selected = plan(&request(root.path(), runtime), &CountRequest::default()).unwrap();
            assert_eq!(selected.status, "blocked");
            assert!(
                selected
                    .reason
                    .as_deref()
                    .is_some_and(|reason| !reason.is_empty())
            );
            assert!(!root.path().join("capture").exists());
        }
    }

    #[test]
    fn actual_python_capture_passes_shared_finalization_and_observation_schema() {
        let Some(executable) = std::env::var_os("HUNIN_TEST_COUNTS_PYTHON") else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let mut request = request(root.path(), "python");
        request.command = vec![
            executable.to_string_lossy().into_owned(),
            "-c".into(),
            "def leaf(): return 9\nfor _ in range(17): leaf()\nprint('ok')".into(),
        ];
        let options: CountRequest =
            serde_json::from_value(json!({"functions":[{"name":"leaf"}]})).unwrap();
        let selected = plan(&request, &options).unwrap();
        assert_eq!(selected.status, "ready", "{:?}", selected.reason);
        assert!(!request.artifact_dir.exists());
        let capture = prepare(&request, &options).unwrap();
        let output = crate::host::probe::output(
            std::process::Command::new(&capture.command[0])
                .args(&capture.command[1..])
                .envs(&capture.env)
                .current_dir(&request.cwd),
            None,
            std::time::Duration::from_secs(30),
            65536,
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"ok\n");
        capture
            .finalize(&request)
            .unwrap()
            .completion
            .ensure_complete()
            .unwrap();
        let records =
            std::fs::read_to_string(request.artifact_dir.join("calls.calls.jsonl")).unwrap();
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../protocol/record.schema.json")).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let records = records
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        for record in &records {
            validator.validate(record).unwrap();
        }
        assert!(
            records
                .iter()
                .any(|record| record["name"] == "function.calls" && record["value"] == 17)
        );
    }

    #[test]
    fn missing_python_compiler_is_an_explicit_plan_failure_without_capture_files() {
        let Some(executable) = std::env::var_os("HUNIN_TEST_COUNTS_PYTHON") else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let mut request = request(root.path(), "python");
        request.command = vec![
            executable.to_string_lossy().into_owned(),
            "-c".into(),
            "pass".into(),
        ];
        request.env.insert("PATH".into(), String::new());
        let selected = plan(&request, &CountRequest::default()).unwrap();
        assert_eq!(selected.status, "blocked");
        assert!(
            selected
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("compiler unavailable")),
            "{:?}",
            selected.reason
        );
        assert!(!request.artifact_dir.exists());
    }

    #[test]
    #[cfg(unix)]
    fn declared_cpp_recipe_counts_exact_entries_and_preserves_original_binary() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("target.cpp");
        let original = root.path().join("original");
        std::fs::write(&source, "#include <cstdio>\nextern \"C\" __attribute__((noinline)) unsigned leaf(unsigned x){return x+3;}\nint main(){unsigned sum=0;for(unsigned i=0;i<37;i++)sum+=leaf(i);std::printf(\"%u\\n\",sum);}\n").unwrap();
        let output = crate::host::probe::standard(
            std::process::Command::new("clang++")
                .arg("-O2")
                .arg(&source)
                .arg("-o")
                .arg(&original),
        )
        .unwrap();
        assert!(output.status.success());
        let before = std::fs::read(&original).unwrap();
        let mut request = request(root.path(), "cpp");
        request.command = vec![original.to_string_lossy().into_owned()];
        let options: CountRequest = serde_json::from_value(json!({"allow_rebuild":true,"build_command":["clang++","-O2",source,"-o","{output}"],"functions":[{"name":"leaf"}]})).unwrap();
        assert_eq!(plan(&request, &options).unwrap().status, "ready");
        let capture = prepare(&request, &options).unwrap();
        let output = crate::host::probe::output(
            std::process::Command::new(&capture.command[0])
                .args(&capture.command[1..])
                .envs(&capture.env)
                .current_dir(&request.cwd),
            None,
            std::time::Duration::from_secs(10),
            65536,
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"777\n");
        capture
            .finalize(&request)
            .unwrap()
            .completion
            .ensure_complete()
            .unwrap();
        let records =
            std::fs::read_to_string(request.artifact_dir.join("calls.calls.jsonl")).unwrap();
        let records = records
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert!(
            records
                .iter()
                .any(|record| record["name"] == "function.calls"
                    && record["attributes"]["function"]["name"] == "leaf"
                    && record["value"] == 37),
            "{records:?}"
        );
        assert_eq!(std::fs::read(original).unwrap(), before);
    }

    #[test]
    #[cfg(unix)]
    fn stripped_private_functions_keep_distinct_identities_and_exact_global_selection() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("symbol_identity.cpp");
        let original = root.path().join("original");
        std::fs::write(&source, r#"#include <cstdio>
extern "C" __attribute__((noinline, used)) unsigned global_leaf(unsigned x) { return x + 3; }
static __attribute__((noinline, used)) unsigned static_leaf(unsigned x) { return x + 5; }
extern "C" __attribute__((noinline, used, visibility("hidden"))) unsigned hidden_leaf(unsigned x) { return x + 7; }
int main() {
    unsigned sum = 0;
    for (unsigned i = 0; i < 7; ++i) sum += global_leaf(i);
    for (unsigned i = 0; i < 11; ++i) sum += static_leaf(i);
    for (unsigned i = 0; i < 13; ++i) sum += hidden_leaf(i);
    std::printf("%u\n", sum);
}
"#).unwrap();
        let output = crate::host::probe::standard(
            std::process::Command::new("clang++")
                .arg("-O2")
                .arg(&source)
                .arg("-o")
                .arg(&original),
        )
        .unwrap();
        assert!(output.status.success());
        for selected in [false, true] {
            let mut request = request(root.path(), "cpp");
            request.command = vec![original.to_string_lossy().into_owned()];
            request.artifact_dir = root.path().join(if selected { "selected" } else { "all" });
            let options: CountRequest = serde_json::from_value(json!({"allow_rebuild":true,"build_command":["clang++","-O2","-Wl,-x",source,"-o","{output}"],"functions":if selected {json!([{"name":"global_leaf"}])}else{json!([])}})).unwrap();
            let capture = prepare(&request, &options).unwrap();
            let output = crate::host::probe::output(
                std::process::Command::new(&capture.command[0])
                    .args(&capture.command[1..])
                    .envs(&capture.env)
                    .current_dir(&request.cwd),
                None,
                std::time::Duration::from_secs(10),
                65536,
            )
            .unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, b"321\n");
            let data =
                std::fs::read_to_string(request.artifact_dir.join("calls.calls.jsonl")).unwrap();
            if let Some(directory) = std::env::var_os("HUNIN_TEST_COUNT_SYMBOL_EVIDENCE") {
                let directory = std::path::PathBuf::from(directory);
                std::fs::create_dir_all(&directory).unwrap();
                let scope = if selected { "selected" } else { "all" };
                std::fs::write(directory.join(format!("{scope}.calls.jsonl")), &data).unwrap();
                std::fs::copy(
                    request.artifact_dir.join("calls.complete.json"),
                    directory.join(format!("{scope}.complete.json")),
                )
                .unwrap();
                std::fs::copy(&source, directory.join("symbol_identity.cpp")).unwrap();
                std::fs::write(
                    directory.join(format!("{scope}.plan.json")),
                    serde_json::to_vec_pretty(&capture).unwrap(),
                )
                .unwrap();
            }
            let completion = capture.finalize(&request).unwrap().completion;
            assert!(
                completion.ensure_complete().is_ok(),
                "{:?}\n{data}",
                completion.diagnostics
            );
            let rows = data
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .filter(|row| row["kind"] == "metric")
                .collect::<Vec<_>>();
            assert!(
                rows.iter()
                    .any(|row| row["attributes"]["function"]["name"] == "global_leaf"
                        && row["value"] == 7),
                "{rows:?}"
            );
            if selected {
                assert_eq!(
                    rows.len(),
                    1,
                    "private entries must not be attributed to a neighboring exported function: {rows:?}"
                );
            } else {
                for (name, count) in [("static_leaf", 11), ("hidden_leaf", 13)] {
                    let row = rows
                        .iter()
                        .find(|row| row["value"] == count)
                        .unwrap_or_else(|| panic!("missing {name}: {rows:?}"));
                    let identity = row["attributes"]["function"]["name"].as_str().unwrap();
                    assert!(
                        identity.contains(name) || row["attributes"]["address"] == identity,
                        "non-exact symbol must fall back to its own address, never another function: {row}"
                    );
                }
            }
        }
    }
}

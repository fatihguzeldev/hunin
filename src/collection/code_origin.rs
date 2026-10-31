//! V8 code-capture prerequisites and declared delivery; neither proves mapping fidelity.
use crate::host::{probe, resolve_launch_executable, tool_path};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
    process::Command,
    time::Duration,
};

pub(crate) fn prerequisites(
    executable: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    compiler_export: bool,
) -> Result<()> {
    let executable = resolve_launch_executable(executable, cwd, env)
        .context("the selected Node executable is unavailable")?;
    let output = probe::output(
        Command::new(tool_path(&executable))
            .arg("--v8-options")
            .env_remove("NODE_OPTIONS")
            .env_remove("BUN_OPTIONS"),
        None,
        Duration::from_secs(2),
        512 * 1024,
    )?;
    ensure!(
        output.status.success(),
        "the selected executable did not expose V8 options"
    );
    let text = std::str::from_utf8(&output.stdout).context("V8 options are not UTF-8")?;
    let mut required = vec![
        "--prof",
        "--ll-prof",
        "--logfile",
        "--prof-sampling-interval",
    ];
    if compiler_export {
        required.extend([
            "--trace-turbo",
            "--trace-turbo-graph",
            "--trace-turbo-path",
            "--trace-turbo-cfg-file",
            "--print-wasm-code",
        ]);
    }
    for flag in required {
        ensure!(
            text.lines().any(|line| line
                .trim_start()
                .strip_prefix(flag)
                .is_some_and(|rest| rest.starts_with(" ("))),
            "the selected runtime does not expose {flag}"
        );
    }
    Ok(())
}

pub(super) fn validate_output(
    request: &super::CaptureRequest,
    files: &[PathBuf],
    reads: &mut super::javascript::ValidationReads,
) -> Result<()> {
    let maximum = request
        .code_options
        .clone()
        .unwrap_or_default()
        .max_artifact_bytes;
    let selected = files.iter().collect::<BTreeSet<_>>();
    let mut total = 0u64;
    for path in files {
        reads.check()?;
        let metadata = std::fs::symlink_metadata(path)?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "code-origin output is not a regular retained file"
        );
        total = total
            .checked_add(metadata.len())
            .context("code-origin artifact byte total overflow")?;
    }
    ensure!(
        total <= maximum,
        "code-origin artifact byte budget exceeded: observed {total}, maximum {maximum}; raw bytes are retained"
    );
    let manifests = files
        .iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".v8code.json"))
        })
        .collect::<Vec<_>>();
    ensure!(
        !manifests.is_empty(),
        "code-origin produced no V8 code manifest"
    );
    for path in manifests {
        let manifest: serde_json::Value = serde_json::from_slice(&reads.metadata(path)?)?;
        ensure!(
            manifest["schema_version"] == 1 && manifest["run_id"] == request.run_id,
            "code-origin manifest identity mismatch"
        );
        ensure!(
            manifest["pid"].as_u64().is_some_and(|pid| pid > 0),
            "code-origin manifest process identity missing"
        );
        ensure!(
            manifest["status"] == "target_exit_observed",
            "code-origin target exit was not observed"
        );
        for key in ["node", "v8", "arch", "platform", "exec_path"] {
            ensure!(
                manifest["runtime"][key]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "code-origin runtime {key} missing"
            );
        }
        let diagnostics = manifest["diagnostics"]
            .as_array()
            .context("code-origin diagnostics missing")?;
        ensure!(
            diagnostics.is_empty(),
            "code-origin collector reported incomplete custody: {}",
            serde_json::to_string(diagnostics)?
        );
        let mut names = vec![];
        for key in ["logs", "wasm", "compiler_exports"] {
            let rows = manifest[key]
                .as_array()
                .context("code-origin manifest inventory missing")?;
            ensure!(
                rows.len() <= 1024,
                "code-origin inventory entry limit exceeded"
            );
            if key == "logs" {
                ensure!(!rows.is_empty(), "code-origin native tick log missing");
            }
            for row in rows {
                names.push(
                    row["path"]
                        .as_str()
                        .context("code-origin inventory filename missing")?,
                );
                if key == "logs" {
                    names.push(
                        row["ll_path"]
                            .as_str()
                            .context("code-origin low-level code log missing")?,
                    );
                }
            }
        }
        if let Some(name) = manifest["published_code"]["path"].as_str() {
            names.push(name);
        }
        for name in names {
            ensure!(
                !name.contains(['\\', ':'])
                    && matches!(
                        Path::new(name).components().collect::<Vec<_>>().as_slice(),
                        [Component::Normal(_)]
                    ),
                "code-origin sidecar must have one safe filename"
            );
            let sidecar = path.parent().unwrap().join(name);
            ensure!(
                selected.contains(&sidecar),
                "code-origin required sidecar was not retained: {name}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(root: &Path) -> (super::super::CaptureRequest, Vec<PathBuf>) {
        let request=serde_json::from_value(json!({"command":["node","work.mjs"],"runtime":"node","cwd":root,"artifact_dir":root,"run_id":"capture","capture_mode":"code-origin"})).unwrap();
        let manifest = json!({"schema_version":1,"run_id":"capture","pid":123,"runtime":{"node":"v22.16.0","v8":"12.4","arch":"arm64","platform":"darwin","exec_path":"/selected/node"},"status":"target_exit_observed","diagnostics":[],"logs":[{"path":"ticks.log","ll_path":"ticks.ll"}],"wasm":[],"compiler_exports":[]});
        std::fs::write(
            root.join("code.v8code.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("ticks.log"), b"opaque ticks").unwrap();
        std::fs::write(root.join("ticks.ll"), b"opaque code bytes").unwrap();
        let files = ["code.v8code.json", "ticks.log", "ticks.ll"]
            .map(|name| root.join(name))
            .to_vec();
        (request, files)
    }

    #[test]
    fn code_delivery_checks_only_selected_inventory_and_preserves_missing_evidence() {
        let root = tempfile::tempdir().unwrap();
        let (request, mut files) = fixture(root.path());
        validate_output(
            &request,
            &files,
            &mut super::super::javascript::ValidationReads::new(None),
        )
        .unwrap();
        files.pop();
        let error = validate_output(
            &request,
            &files,
            &mut super::super::javascript::ValidationReads::new(None),
        )
        .unwrap_err();
        assert!(error.to_string().contains("not retained"));
        assert_eq!(
            std::fs::read(root.path().join("ticks.ll")).unwrap(),
            b"opaque code bytes"
        );
    }

    #[test]
    fn oversized_code_delivery_is_incomplete_without_truncating_raw_bytes() {
        let root = tempfile::tempdir().unwrap();
        let (mut request, files) = fixture(root.path());
        request.code_options = Some(super::super::CodeOptions {
            max_artifact_bytes: 1,
            ..Default::default()
        });
        let error = validate_output(
            &request,
            &files,
            &mut super::super::javascript::ValidationReads::new(None),
        )
        .unwrap_err();
        assert!(error.to_string().contains("byte budget exceeded"));
        assert_eq!(
            std::fs::read(root.path().join("ticks.log")).unwrap(),
            b"opaque ticks"
        );
    }
}

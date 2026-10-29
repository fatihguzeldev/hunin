use super::{CapturePlan, CaptureRequest, assets, probe};
use crate::host::{resolve_launch_executable, tool_path};
use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use std::{collections::BTreeMap, env, fs, path::Path, process::Command, time::Duration};

const PLANNER_PROTOCOL: u64 = 1;

/// Run the bundled host adapter with the selected runtime, separately from its target.
pub(super) fn prepare(request: &CaptureRequest) -> Result<CapturePlan> {
    let name = request
        .command
        .first()
        .context("a target command is required")?;
    let name = tool_path(Path::new(name));
    let executable = resolve_launch_executable(
        &name.to_string_lossy(),
        &tool_path(&request.cwd),
        &request.env,
    )
    .with_context(|| {
        format!(
            "selected {} runtime executable was not found: {}",
            request.runtime,
            name.display()
        )
    })?;
    let mut command = Command::new(tool_path(&executable));
    match request.runtime.as_str() {
        "node" => {}
        "bun" => {
            // Keep project bunfig/.env/preloads out of the owned planner process.
            // The target still receives its original runtime options and config.
            let config = request.artifact_dir.join("planner.bunfig.toml");
            fs::write(&config, b"preload = []\n")?;
            // A separate --config value becomes Bun's entrypoint; use one argument.
            let mut config_argument = std::ffi::OsString::from("--config=");
            config_argument.push(tool_path(&config));
            command
                .arg(config_argument)
                .args(["--no-env-file", "--no-install", "--no-macros"]);
        }
        "deno" => {
            // These capabilities belong only to this bundled host adapter.
            // They are never appended to the returned target command.
            command.args([
                "run",
                "--no-config",
                "--no-lock",
                "--no-npm",
                "--no-remote",
                "--allow-read",
                "--allow-write",
                "--allow-env",
                "--allow-run",
                "--no-prompt",
            ]);
        }
        runtime => bail!("unsupported JavaScript controller runtime: {runtime}"),
    }
    command
        .arg(tool_path(&assets().join("collectors/javascript/plan.mjs")))
        .env_remove("NODE_OPTIONS")
        .env_remove("BUN_OPTIONS");
    let inherited_options = ["NODE_OPTIONS", "BUN_OPTIONS"]
        .into_iter()
        .filter_map(|key| {
            env::var_os(key).map(|value| (key.to_owned(), value.to_string_lossy().into_owned()))
        })
        .collect::<BTreeMap<_, _>>();
    let mut input = serde_json::to_value(request)?;
    input["inherited_runtime_options"] = json!(inherited_options);
    let output = probe::output(
        &mut command,
        Some(serde_json::to_vec(&input)?),
        Duration::from_secs(15),
        128 * 1024,
    )
    .with_context(|| {
        format!(
            "{} capture planner could not run in selected executable {}",
            request.runtime,
            executable.display()
        )
    })?;
    ensure!(
        output.status.success(),
        "{} capture planner in selected executable {} exited {}: {}",
        request.runtime,
        executable.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    decode_plan(&output.stdout)
}

fn decode_plan(bytes: &[u8]) -> Result<CapturePlan> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).context("invalid JavaScript capture plan")?;
    ensure!(
        value["planner_protocol"] == PLANNER_PROTOCOL,
        "JavaScript planner protocol is unavailable or incompatible; use matching Hunin collector assets"
    );
    serde_json::from_value(value).context("invalid JavaScript capture plan")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_or_invalid_planner_protocol_cannot_silently_drop_target_options() {
        let plan = json!({"command":["node","app.js"],"runtime_version":null,"output":{"kind":"profiles","format":"v8-cpuprofile"}});
        assert!(decode_plan(&serde_json::to_vec(&plan).unwrap()).is_err());
        let mut current = plan;
        current["planner_protocol"] = json!(PLANNER_PROTOCOL);
        assert_eq!(
            decode_plan(&serde_json::to_vec(&current).unwrap())
                .unwrap()
                .command,
            ["node", "app.js"]
        );
        current["planner_protocol"] = json!("1");
        assert!(decode_plan(&serde_json::to_vec(&current).unwrap()).is_err());
    }
}

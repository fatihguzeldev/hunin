use crate::host::{probe, resolve_launch_executable};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub(in crate::importers) struct Decoded {
    pub text: String,
    pub provenance: Value,
    pub diagnostics: Vec<String>,
}

#[derive(Deserialize)]
struct Environment {
    #[serde(rename = "GOVERSION")]
    version: String,
}

fn digest(path: &Path) -> Result<String> {
    let mut input = fs::File::open(path)
        .with_context(|| format!("read Go decoder executable {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            return Ok(format!("{:x}", hash.finalize()));
        }
        hash.update(&buffer[..count]);
    }
}

fn resolve(name: &str) -> Option<PathBuf> {
    resolve_launch_executable(name, &env::current_dir().ok()?, &BTreeMap::new())
}

pub(in crate::importers) fn read(path: &Path, deadline: Option<Instant>) -> Result<Decoded> {
    let metadata: Value = fs::read(path.parent().unwrap_or(Path::new(".")).join("go-tool.json"))
        .ok()
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()?
        .unwrap_or(json!({}));
    let (go, selection) = if let Some(name) = env::var_os("HUNIN_GO") {
        let supplied = PathBuf::from(&name);
        let selected = if supplied.is_absolute() || supplied.components().count() > 1 {
            let path = env::current_dir()?.join(&supplied);
            path.is_file().then_some(path)
        } else {
            name.to_str().and_then(resolve)
        };
        (
            selected.context("explicit HUNIN_GO decoder executable is unavailable")?,
            "explicit_override",
        )
    } else if let Some(go) = metadata["executable"].as_str().and_then(resolve) {
        (go, "retained_tool_hint")
    } else {
        (
            resolve("go").context(
                "Go execution trace import requires a compatible Go toolchain on PATH or HUNIN_GO",
            )?,
            "available_toolchain",
        )
    };
    read_with_tool(
        path,
        &go,
        selection,
        metadata.get("executable").is_some(),
        deadline,
    )
}

fn read_with_tool(
    path: &Path,
    go: &Path,
    selection: &str,
    has_capture_hint: bool,
    deadline: Option<Instant>,
) -> Result<Decoded> {
    let began = Instant::now();
    let remaining = || {
        let own = Duration::from_secs(60)
            .checked_sub(began.elapsed())
            .filter(|value| !value.is_zero())
            .context("Go trace decoder deadline elapsed")?;
        let remaining = deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()).min(own))
            .unwrap_or(own);
        ensure!(
            !remaining.is_zero(),
            "Go trace decoder normalization deadline elapsed"
        );
        Ok(remaining)
    };
    let go_hash = digest(go)?;
    let environment = probe::output(
        Command::new(go).args(["env", "-json", "GOVERSION"]),
        None,
        remaining()?,
        32 * 1024,
    )?;
    ensure!(
        environment.status.success(),
        "inspect Go trace toolchain: {}",
        String::from_utf8_lossy(&environment.stderr)
    );
    let environment: Environment = serde_json::from_slice(&environment.stdout)
        .context("Go decoder environment is missing GOVERSION")?;
    ensure!(
        !environment.version.is_empty(),
        "Go decoder environment has no identified toolchain"
    );
    // Newer Go distributions build selected tools into the build cache rather
    // than shipping them under GOTOOLDIR. Ask the launcher for its actual tool.
    let located = probe::output(
        Command::new(go).args(["tool", "-n", "trace"]),
        None,
        remaining()?,
        32 * 1024,
    )?;
    ensure!(
        located.status.success(),
        "locate Go trace tool: {}",
        String::from_utf8_lossy(&located.stderr)
    );
    let trace = PathBuf::from(
        std::str::from_utf8(&located.stdout)
            .context("Go trace tool path is not UTF-8")?
            .trim(),
    );
    ensure!(
        trace.is_absolute() && trace.is_file(),
        "selected Go trace decoder is not a regular executable file"
    );
    let trace_hash = digest(&trace)?;
    let mut mode = "-d=parsed";
    // Invoke the exact tool that was inspected, so a second Go launcher
    // resolution cannot silently select another toolchain for this decode.
    let mut output = probe::output(
        Command::new(&trace).arg(mode).arg(path),
        None,
        remaining()?,
        64 * 1024 * 1024,
    )?;
    if !output.status.success()
        && String::from_utf8_lossy(&output.stderr).contains("invalid debug mode")
    {
        mode = "-d=1";
        output = probe::output(
            Command::new(&trace).arg(mode).arg(path),
            None,
            remaining()?,
            64 * 1024 * 1024,
        )?;
    }
    if !output.status.success() {
        bail!(
            "Go execution trace decode failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    ensure!(
        go_hash == digest(go)? && trace_hash == digest(&trace)?,
        "Go decoder executables changed during normalization"
    );
    remaining()?;
    let text =
        String::from_utf8(output.stdout).context("Go trace decoder emitted non-UTF8 data")?;
    let provenance = json!({
        "schema_version":1,
        "identity":{
            "decoder":"hunin-go-trace-v1",
            "go_version":environment.version,
            "launcher_sha256":go_hash,
            "trace_tool_sha256":trace_hash,
            "arguments":[mode],
        },
        "evidence":{
            "launcher":go,
            "resolved_launcher":go.canonicalize()?,
            "trace_tool":trace,
            "selection":selection,
            "scope":"Observed Go launcher and selected standalone trace tool bytes/version/options across decode; not a complete OS/toolchain environment snapshot or adversarial file replacement guarantee.",
        },
    });
    let diagnostics = if has_capture_hint && selection == "available_toolchain" {
        vec!["Retained Go capture tool is unavailable; offline trace uses the identified available decoder toolchain. Capture and decoder identities remain separate.".into()]
    } else {
        vec![]
    };
    Ok(Decoded {
        text,
        provenance,
        diagnostics,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn changed_tool_bytes_cannot_publish_a_decoder_identity() {
        let directory = tempfile::tempdir().unwrap();
        let trace = directory.path().join("trace");
        fs::write(
            &trace,
            "#!/bin/sh\nprintf '\\n# changed' >> \"$0\"\nprintf ''\n",
        )
        .unwrap();
        fs::set_permissions(&trace, fs::Permissions::from_mode(0o755)).unwrap();
        let go = directory.path().join("go");
        // This executable only supplies a controlled Go environment. The
        // selected trace process really modifies its bytes during execution.
        let environment = json!({"GOVERSION":"go1.24.13"});
        let json = serde_json::to_string(&environment)
            .unwrap()
            .replace('\'', "'\\''");
        let trace_path = trace.to_string_lossy().replace('\'', "'\\''");
        fs::write(&go, format!("#!/bin/sh\nif [ \"$1\" = env ]; then printf '%s' '{json}'; else printf '%s' '{trace_path}'; fi\n")).unwrap();
        fs::set_permissions(&go, fs::Permissions::from_mode(0o755)).unwrap();
        let error = read_with_tool(
            &directory.path().join("unused"),
            &go,
            "control",
            false,
            None,
        )
        .err()
        .unwrap();
        assert!(
            error.to_string().contains("executables changed"),
            "{error:#}"
        );
    }

    #[test]
    fn legacy_debug_fallback_binds_the_mode_that_actually_decoded() {
        let directory = tempfile::tempdir().unwrap();
        let trace = directory.path().join("trace");
        fs::write(&trace, "#!/bin/sh\nif [ \"$1\" = -d=parsed ]; then printf 'invalid debug mode' >&2; exit 1; fi\nprintf 'legacy-output'\n").unwrap();
        fs::set_permissions(&trace, fs::Permissions::from_mode(0o755)).unwrap();
        let go = directory.path().join("go");
        let environment = serde_json::to_string(&json!({"GOVERSION":"go1.24.13"})).unwrap();
        let trace_path = trace.to_string_lossy().replace('\'', "'\\''");
        fs::write(&go, format!("#!/bin/sh\nif [ \"$1\" = env ]; then printf '%s' '{environment}'; else printf '%s' '{trace_path}'; fi\n")).unwrap();
        fs::set_permissions(&go, fs::Permissions::from_mode(0o755)).unwrap();
        let decoded = read_with_tool(
            &directory.path().join("unused"),
            &go,
            "control",
            false,
            None,
        )
        .unwrap();
        assert_eq!(decoded.text, "legacy-output");
        assert_eq!(decoded.provenance["identity"]["arguments"], json!(["-d=1"]));
    }
}

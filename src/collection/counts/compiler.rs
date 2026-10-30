use crate::{
    collection::{CaptureOutput, CapturePlan, CaptureRequest},
    host::{assets, probe, visit_regular},
    model::calls::CountRequest,
};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

fn digest(path: &Path, maximum: u64) -> Result<String> {
    let mut hash = Sha256::new();
    let mut read_bytes = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    visit_regular(
        path,
        maximum,
        &mut read_bytes,
        || {
            ensure!(
                Instant::now() < deadline,
                "count collector provenance read exceeded its 10s budget"
            );
            Ok(())
        },
        |bytes| {
            hash.update(bytes);
            Ok(())
        },
    )?;
    Ok(format!("{:x}", hash.finalize()))
}

pub(super) fn limitations() -> Vec<String> {
    ["A declared Unix C/C++ compiler invocation with -o {output} and explicit rebuild permission is required; the original executable is preserved", "Compiler entry and exit hooks change execution; only entry counters are aggregated and no per-call timestamps or I/O are collected", "Counts cover the rebuilt image's instrumented functions; linked libraries not compiled with the flags, child processes, inlined source calls without emitted hooks and pre-initializer work are not a complete program inventory", "Functions use exact raw linker names where dladdr exposes them, otherwise address identity; source file and line are unknown", "Selectors filter aggregated observations; every instrumented entry still executes a callback. Concurrent process shutdown and all platform/compiler combinations are not verified"].map(str::to_owned).into()
}
pub(super) fn prerequisite(request: &CaptureRequest, options: &CountRequest) -> Result<String> {
    ensure!(
        cfg!(unix),
        "compiler-hook counting currently requires a verified Unix compiler route; Windows existing-binary counting is not established"
    );
    ensure!(
        options.allow_rebuild,
        "C/C++ automatic counting requires explicit allow_rebuild and a declared compiler build_command; existing binaries remain unchanged"
    );
    let command = options
        .build_command
        .as_ref()
        .context("C/C++ counting requires a declared compiler build_command with -o {output}")?;
    ensure!(
        command
            .windows(2)
            .filter(|pair| pair[0] == "-o" && pair[1] == "{output}")
            .count()
            == 1
            && command.iter().filter(|arg| arg.as_str() == "-o").count() == 1
            && command
                .iter()
                .filter(|arg| arg.contains("{output}"))
                .count()
                == 1,
        "compiler build_command must have exactly one -o {{output}} argument pair"
    );
    ensure!(
        command.iter().all(|arg| !arg.starts_with('@')
            && !["-c", "-S", "-E", "-shared", "-dynamiclib", "-emit-llvm"].contains(&arg.as_str())),
        "only a direct executable compiler/link invocation is supported; response files, object-only and shared-library recipes are unsupported"
    );
    ensure!(
        command
            .iter()
            .all(|arg| !(arg.starts_with("-o") && arg != "-o")
                && !arg.starts_with("--output")
                && arg != "-Xlinker"
                && !arg.starts_with("-Wl,-o")
                && !arg.starts_with("-Wl,--output")),
        "compiler output overrides outside -o {{output}} are unsupported"
    );
    ensure!(
        options
            .functions
            .iter()
            .all(|function| function.file.is_none()
                && function.line.is_none()
                && !function.name.contains('\n')),
        "compiler-hook selectors support exact linker names only; source file/line selectors need a verified source resolver"
    );
    ensure!(
        request.run_id.len() <= 1024 && !request.run_id.contains('\0'),
        "native count run identity exceeds its bounded metadata contract"
    );
    let output = probe::standard(
        Command::new(&command[0])
            .envs(&request.env)
            .current_dir(&request.cwd)
            .arg("--version"),
    )?;
    let version = String::from_utf8_lossy(&output.stdout).into_owned();
    ensure!(
        output.status.success()
            && (version.to_lowercase().contains("clang")
                || version.to_lowercase().contains("gcc")
                || version.to_lowercase().contains("free software foundation")),
        "build_command must invoke an available Clang or GCC compiler directly"
    );
    Ok(version.trim().into())
}
pub(super) fn prepare(request: &CaptureRequest, options: &CountRequest) -> Result<CapturePlan> {
    let version = prerequisite(request, options)?;
    let recipe = options.build_command.as_ref().unwrap();
    let object = request.artifact_dir.join("count-hooks.o");
    let binary = request.artifact_dir.join("count-target");
    ensure!(
        !binary.exists() && !binary.is_symlink(),
        "preserving existing counted executable {}; use a new recording",
        binary.display()
    );
    let source = assets().join("collectors/counts/hooks.c");
    // Explicit language mode keeps clang++/g++ from interpreting the hook runtime as C++.
    let output = probe::output(
        Command::new(&recipe[0])
            .envs(&request.env)
            .current_dir(&request.cwd)
            .args(["-x", "c", "-std=c11", "-O2", "-c"])
            .arg(&source)
            .arg(format!("-DHUNIN_COUNT_MAX={}", options.max_functions))
            .arg("-o")
            .arg(&object),
        None,
        Duration::from_secs(30),
        128 * 1024,
    )?;
    ensure!(
        output.status.success(),
        "compile count hook runtime: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut build = recipe
        .iter()
        .map(|arg| {
            if arg == "{output}" {
                binary.to_string_lossy().into_owned()
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>();
    build.extend([
        "-finstrument-functions".into(),
        "-x".into(),
        "none".into(),
        object.to_string_lossy().into_owned(),
    ]);
    if cfg!(target_os = "macos") {
        build.push("-Wl,-export_dynamic".into());
    } else {
        build.extend([
            "-Wl,--export-dynamic".into(),
            "-ldl".into(),
            "-pthread".into(),
        ]);
    }
    let output = probe::output(
        Command::new(&build[0])
            .envs(&request.env)
            .current_dir(&request.cwd)
            .args(&build[1..]),
        None,
        Duration::from_secs(30),
        128 * 1024,
    )?;
    ensure!(
        output.status.success() && binary.is_file(),
        "build isolated counted executable: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        Path::new(&request.command[0]).canonicalize().ok().as_ref()
            != Some(&binary.canonicalize()?),
        "count output must not replace the original executable"
    );
    let profile = request.artifact_dir.join("calls.calls.jsonl");
    let completion = request.artifact_dir.join("calls.complete.json");
    fs::write(
        &completion,
        serde_json::to_vec(
            &json!({"schema_version":1,"run_id":request.run_id,"method":"compiler-hooks","status":"planned"}),
        )?,
    )?;
    let env = BTreeMap::from([
        (
            "HUNIN_COUNT_OUTPUT".into(),
            profile.to_string_lossy().into_owned(),
        ),
        (
            "HUNIN_COUNT_COMPLETION".into(),
            completion.to_string_lossy().into_owned(),
        ),
        ("HUNIN_COUNT_RUN_ID".into(), request.run_id.clone()),
        (
            "HUNIN_COUNT_MAX_BYTES".into(),
            options.max_output_bytes.to_string(),
        ),
        (
            "HUNIN_COUNT_FUNCTIONS".into(),
            options
                .functions
                .iter()
                .map(|function| function.name.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    ]);
    let mut command = vec![binary.to_string_lossy().into_owned()];
    command.extend_from_slice(&request.command[1..]);
    Ok(CapturePlan {
        command,
        env,
        diagnostics: limitations(),
        runtime_version: Some(version),
        collector_identity: Some(
            json!({"name":"compiler-hooks","hook_source_sha256":digest(&source, 1024 * 1024)?,"target_sha256":digest(&binary, 256 * 1024 * 1024)?,"build_command":build}),
        ),
        session_identity: BTreeMap::new(),
        output: CaptureOutput::FunctionCounts {
            method: "compiler-hooks".into(),
            maximum_bytes: options.max_output_bytes,
        },
    })
}

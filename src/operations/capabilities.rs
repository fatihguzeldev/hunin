use crate::collection::{
    CaptureAdapter, LaunchExecutableKind, capture_modes, python_cpu_prerequisite,
};
use crate::host::{probe, tool};
use serde_json::{Value, json};
use std::{env, process::Command, sync::Arc, time::Duration};

const CPU_PROGRESS_MECHANISM: &str = "os-thread-cpu-progress";
const CPU_PROGRESS_BASIS: &str = "measured OS thread CPU counter progress statistically attributed to bounded Python stack snapshots";

fn checked(command: &mut Command) -> anyhow::Result<String> {
    let output = probe::output(command, None, Duration::from_secs(2), 32 * 1024)?;
    if !output.status.success() {
        anyhow::bail!(
            "probe exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn permission() -> Value {
    #[cfg(target_os = "linux")]
    {
        // Test one inactive user-only CPU counter, without sampling any target.
        #[repr(C)]
        struct Attr {
            kind: u32,
            size: u32,
            config: u64,
            period: u64,
            sample_type: u64,
            read_format: u64,
            flags: u64,
            wakeup: u32,
            bp_type: u32,
            config1: u64,
        }
        let attr = Attr {
            kind: 1,
            size: std::mem::size_of::<Attr>() as u32,
            config: 0,
            period: 0,
            sample_type: 0,
            read_format: 0,
            flags: 1 | (1 << 5) | (1 << 6),
            wakeup: 0,
            bp_type: 0,
            config1: 0,
        };
        let fd = unsafe { libc::syscall(libc::SYS_perf_event_open, &attr, 0, -1, -1, 0) };
        if fd >= 0 {
            unsafe {
                libc::close(fd as i32);
            };
            return json!({"state":"basic_permission_observed","probe":"inactive self-process perf software CPU counter; complete samply recording remains unverified"});
        }
        json!({"state":"permission_unavailable","error":std::io::Error::last_os_error().to_string(),"perf_event_paranoid":std::fs::read_to_string("/proc/sys/kernel/perf_event_paranoid").ok().map(|s|s.trim().to_owned())})
    }
    #[cfg(windows)]
    {
        let Some(xperf) = crate::collection::native::tool::xperf_path() else {
            return json!({"state":"native_prerequisite_missing","prerequisite":"xperf.exe","error":"Install Windows Performance Toolkit (WPT) from the Windows ADK; xperf.exe was absent from PATH and the standard ProgramFiles(x86) installation","setup_url":"https://learn.microsoft.com/en-us/windows-hardware/test/wpt/"});
        };
        return match checked(Command::new("whoami.exe").arg("/groups")) {
            Ok(groups) => {
                json!({"state":if groups.contains("S-1-16-12288")||groups.contains("S-1-16-16384"){"elevated_token_observed"}else{"elevation_required"},"xperf_executable":xperf,"probe":"bounded current-token check and WPT executable discovery; actual ETW session not started; occupied NT Kernel Logger causes actionable refusal"})
            }
            Err(error) => json!({"state":"permission_unverified","error":format!("{error:#}")}),
        };
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    json!({"state":"target_permission_unverified","probe":"macOS task access/signing depends on the measured executable; discovery does not attach or request permission"})
}

fn version_pair(runtime: &str, text: &str) -> Option<(u64, u64)> {
    let first = text.lines().next()?.trim();
    let version = match runtime {
        "node" => first.strip_prefix('v')?,
        "bun" => first,
        "deno" => first.strip_prefix("deno ")?,
        _ => return None,
    };
    let version = if runtime == "deno"
        && let Some((version, details)) = version.split_once(" (")
    {
        let details = details.strip_suffix(')')?;
        if details.is_empty() || details.contains([')', '\r', '\n']) {
            return None;
        }
        version
    } else {
        version
    };
    let valid_suffix = |value: &str| {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    };
    let version = if let Some((version, build)) = version.split_once('+') {
        valid_suffix(build).then_some(version)?
    } else {
        version
    };
    let version = if let Some((version, prerelease)) = version.split_once('-') {
        valid_suffix(prerelease).then_some(version)?
    } else {
        version
    };
    let number = |value: &str| -> Option<u64> {
        if !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let number = value.parse().ok()?;
        (number <= 9_007_199_254_740_991).then_some(number)
    };
    let mut parts = version.split('.');
    let major = number(parts.next()?)?;
    let minor = number(parts.next()?)?;
    number(parts.next()?)?;
    parts.next().is_none().then_some((major, minor))
}

fn runtime(runtime: &str, names: &[&str], permissions: &Value) -> Value {
    let found = if runtime == "native" {
        crate::collection::native::tool::resolve()
    } else {
        names.iter().find_map(|name| tool(name))
    };
    let mut diagnostics = Vec::new();
    let mut version = None;
    let mut probe_status = "missing";
    if let Some(executable) = found.as_ref() {
        match checked(Command::new(executable).arg(if runtime == "go" {
            "version"
        } else {
            "--version"
        })) {
            Ok(text) => {
                version = Some(text);
                probe_status = "ok";
            }
            Err(error) => {
                probe_status = "failed_or_timed_out";
                diagnostics.push(format!("bounded version probe failed: {error:#}"));
            }
        }
    }
    let mut availability = if found.is_none() {
        "executable_missing"
    } else if version.is_none() {
        "runtime_probe_failed"
    } else {
        "prerequisites_ready"
    };
    let cpu_progress_availability = if matches!(runtime, "cpython" | "pypy") {
        let mut state = availability;
        if let Some(executable) = found.as_ref().filter(|_| version.is_some())
            && let Err(error) = python_cpu_prerequisite(executable, Duration::from_secs(2))
        {
            state = "cpu_counter_prerequisite_failed";
            diagnostics.push(format!("CPU-progress mode prerequisite failed: {error:#}"));
        }
        diagnostics.push("CPU-progress mode uses the standard library; CPU gaps above 100ms and capture latency above 10ms remain unattributed. GIL-holding native calls, native-only threads and undiscovered short-lived threads limit coverage. No vmprof installation is required for this mode.".into());
        Some(state)
    } else {
        None
    };
    let mechanism;
    let mut basis = "statistical stack samples";
    match runtime {
        "cpython" | "pypy" => {
            if cfg!(windows) {
                mechanism = CPU_PROGRESS_MECHANISM;
                basis = CPU_PROGRESS_BASIS;
                availability = cpu_progress_availability.unwrap_or(availability);
            } else {
                mechanism = "vmprof-cpu-timer";
                if let Some(executable) = found.as_ref().filter(|_| version.is_some())
                    && let Err(error)=checked(Command::new(executable).args(["-c","import vmprof,importlib.metadata as m;assert tuple(map(int,m.version('vmprof').split('.')[:2]))==(0,6);print(m.version('vmprof'))"])) {availability="dependency_missing_or_incompatible";diagnostics.push(format!("Selected interpreter requires vmprof0.6.x: {error:#}"));}
                diagnostics.push("CPU-clock samples remain outside an epoch timeline unless a verified mapping exists. The selected interpreter is never replaced.".into());
            }
        }
        "node" | "bun" | "deno" => {
            mechanism = "runtime-cpu-profiler";
            let required = match runtime {
                "node" => (20, 0),
                "bun" => (1, 4),
                _ => (2, 8),
            };
            if let Some(version) = version.as_deref() {
                match version_pair(runtime, version) {
                    Some(value) if value < required => {
                        availability = "runtime_version_unsupported";
                    }
                    Some(_) => {}
                    None => {
                        availability = "runtime_version_unrecognized";
                        diagnostics.push(format!(
                            "The selected {runtime} version output could not establish its supported version floor"
                        ));
                    }
                }
            }
            if runtime == "node"
                && availability == "prerequisites_ready"
                && let Some(executable) = found.as_ref().filter(|_| version.is_some())
            {
                match checked(Command::new(executable).args(["-p","process.allowedNodeEnvironmentFlags.has('--cpu-prof')&&process.allowedNodeEnvironmentFlags.has('--import')"])) {
                        Ok(answer) if answer=="true"=>{},
                        Ok(_)=>{availability="runtime_flags_unsupported";diagnostics.push("This Node runtime cannot accept the required CPU profiler/preload flags in NODE_OPTIONS".into());},
                        Err(error)=>{availability="runtime_flag_probe_failed";diagnostics.push(format!("{error:#}"));}
                    }
            }
            diagnostics.push("The selected runtime executes its bundled host planner. Version/flag prerequisites are checked; discovery does not run that planner or record the target or its workers. Explicit Deno permission denial can leave CPU samples unaligned.".into());
        }
        "graalpy" => {
            mechanism = "truffle-cpusampler";
            if let Some(executable) = found.as_ref().filter(|_| version.is_some()) {
                match checked(Command::new(executable).arg("--help:tools")) {
                    Ok(help) if help.to_lowercase().contains("cpusampler") => {}
                    Ok(_) => {
                        availability = "cpusampler_not_exposed";
                    }
                    Err(error) => {
                        availability = "tool_probe_failed";
                        diagnostics.push(format!("{error:#}"));
                    }
                }
            }
            diagnostics.push("Availability applies to the probed GraalPy mode; native-image and JVM-hosted distributions can expose different tools.".into());
        }
        _ => {
            mechanism = "samply-os-sampling-with-module-lifetimes";
            basis = "OS stack sample weight; CPU/off-CPU semantics and permissions differ by host";
            let sampler = if runtime == "native" {
                found.clone()
            } else {
                crate::collection::native::tool::resolve()
            };
            if let Some(sampler) = sampler {
                if let Err(error) = crate::collection::native::tool::check(
                    &sampler,
                    if runtime == "native" {
                        version.as_deref()
                    } else {
                        None
                    },
                    std::time::Duration::from_secs(2),
                ) {
                    availability = "native_lifetime_extension_missing_or_unverified";
                    diagnostics.push(format!("{error:#}"));
                }
            } else {
                availability = "native_lifetime_collector_missing";
                diagnostics.push("Install Hunin's bundled samply0.13.1-hunin-native1 collector; stock samply lacks the required native module lifetime extension.".into());
            }
            if availability == "prerequisites_ready"
                && !matches!(
                    permissions["state"].as_str(),
                    Some("basic_permission_observed" | "elevated_token_observed")
                )
            {
                availability = match permissions["state"].as_str() {
                    Some("native_prerequisite_missing") => "native_prerequisite_missing",
                    Some("permission_unavailable" | "elevation_required") => "permission_required",
                    _ => "target_permission_unverified",
                };
            }
            diagnostics.push("Basic permissions and prerequisites do not establish that an actual sampling session can attach to the selected target.".into());
        }
    }
    let mut capabilities = Vec::new();
    if version.is_some() {
        capabilities.extend(["sdk-records", "process-resources"]);
    }
    if availability == "prerequisites_ready"
        || cpu_progress_availability == Some("prerequisites_ready")
    {
        capabilities.push("cpu-sampling-prerequisites");
    }
    let modes = match capture_modes(runtime) {
        Ok(registrations) => registrations.iter().map(|mode| {
            let (mode_mechanism, mode_availability, mode_basis) = match mode.launch.adapter {
                CaptureAdapter::GoTestCpu => ("Go runtime pprof", if version.is_some(){"prerequisites_ready"}else{"runtime_probe_failed"}, "aggregated sampled CPU time; one test package per recording"),
                CaptureAdapter::GoTrace => ("Go execution tracer", if version.is_some(){"prerequisites_ready"}else{"runtime_probe_failed"}, "runtime scheduler/GC events; not a CPU stack profile"),
                CaptureAdapter::PythonCpuProgress => (CPU_PROGRESS_MECHANISM, cpu_progress_availability.unwrap_or(availability), CPU_PROGRESS_BASIS),
                _ => (mechanism, availability, basis),
            };
            let default_executable = match mode.executable_kind {
                LaunchExecutableKind::RuntimeInterpreter | LaunchExecutableKind::GoTool => found.as_ref(),
                LaunchExecutableKind::TargetExecutable => None,
            };
            json!({"mode":mode.mode,"launch_runtime":mode.launch.id,"command_requirement":mode.command_requirement,
                "default_executable":default_executable,"selectable":true,
                "mechanism":mode_mechanism,"availability":mode_availability,"basis":mode_basis,"recording_verified":false})
        }).collect::<Vec<_>>(),
        Err(error) => {
            diagnostics.push(format!("Launch selection unavailable: {error:#}"));
            Vec::new()
        }
    };
    if found.is_none() {
        diagnostics.push(format!(
            "No {} executable found; install it on PATH or select an explicit executable",
            names[0]
        ));
    }
    json!({"runtime":runtime,"available":version.is_some(),"executable":found,"version":version,"probe_status":probe_status,"capabilities":capabilities,"modes":modes,"diagnostics":diagnostics,"verification":"bounded prerequisite checks only; actual recording not exercised"})
}

pub fn discover() -> Value {
    let permission = Arc::new(permission());
    let mut workers = Vec::new();
    for (runtime, names) in [
        ("node", vec!["node"]),
        ("bun", vec!["bun"]),
        ("deno", vec!["deno"]),
        ("cpython", vec!["python3", "python"]),
        ("pypy", vec!["pypy3", "pypy"]),
        ("graalpy", vec!["graalpy", "graalpython"]),
        ("go", vec!["go"]),
        ("native", vec!["samply"]),
    ] {
        let permission = permission.clone();
        workers.push((
            runtime,
            std::thread::spawn(move || self::runtime(runtime, &names, &permission)),
        ));
    }
    let runtimes = workers
        .into_iter()
        .map(|(runtime,worker)|worker.join().unwrap_or_else(|_|json!({"runtime":runtime,"available":false,"probe_status":"internal_probe_failure","diagnostics":["Capability worker failed; availability is unknown"]})))
        .collect::<Vec<_>>();
    json!({
        "host":{"os":env::consts::OS,"arch":env::consts::ARCH},
        "native_sampling_permission":*permission,
        "runtimes":runtimes,
        "import_formats":crate::importers::import_formats(),
        "schema_version":1,
        "probe_limits":{"timeout_seconds_per_command":2,"output_bytes_per_stream":32768},
        "verification":"Discovery never labels executable presence as a verified CPU recording.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_version_shapes_match_the_selected_capture_floor_contract() {
        for (runtime, output, expected) in [
            ("node", "v20.0.0", (20, 0)),
            ("node", "v24.0.0-nightly20261029.1", (24, 0)),
            ("bun", "1.4.0", (1, 4)),
            ("bun", "1.4.4-canary.1+build", (1, 4)),
            ("deno", "deno 2.8.0", (2, 8)),
            (
                "deno",
                "deno 2.9.8 (stable, release, test)\r\nv8 14.0.0\r\n",
                (2, 9),
            ),
        ] {
            assert_eq!(version_pair(runtime, output), Some(expected), "{output}");
        }
        for runtime in ["node", "bun", "deno"] {
            for output in ["", "unavailable", "build 2026.10", "v22.16"] {
                assert!(
                    version_pair(runtime, output).is_none(),
                    "{runtime}: {output}"
                );
            }
        }
        for (runtime, output) in [
            ("node", "v999999999999999999999999.0.0"),
            ("bun", "999999999999999999999999.0.0"),
            ("deno", "deno 999999999999999999999999.0.0"),
        ] {
            assert!(version_pair(runtime, output).is_none());
        }
    }

    #[test]
    fn missing_default_probe_keeps_an_explicit_runtime_command_selectable() {
        let capability = runtime(
            "node",
            &["hunin-unavailable-probe-for-capability-contract"],
            &json!({"state":"unknown"}),
        );
        assert_eq!(capability["available"], false);
        assert_eq!(capability["modes"][0]["launch_runtime"], "node");
        assert_eq!(capability["modes"][0]["selectable"], true);
        assert!(capability["modes"][0]["default_executable"].is_null());
        assert_eq!(capability["modes"][0]["recording_verified"], false);
    }

    #[test]
    #[cfg(unix)]
    fn unrecognized_runtime_version_does_not_claim_ready_prerequisites() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("unknown-version-runtime");
        std::fs::write(
            &executable,
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'unavailable\\n'; else printf 'true\\n'; fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        for family in ["node", "bun", "deno"] {
            let capability = runtime(
                family,
                &[executable.to_str().unwrap()],
                &json!({"state":"unknown"}),
            );
            assert_eq!(capability["version"], "unavailable");
            assert_eq!(
                capability["modes"][0]["availability"],
                "runtime_version_unrecognized"
            );
            assert_eq!(capability["modes"][0]["selectable"], true);
            assert_eq!(capability["modes"][0]["recording_verified"], false);
        }
    }

    #[test]
    fn native_mode_requires_an_explicit_target_executable() {
        let capability = runtime(
            "native",
            &["hunin-unavailable-probe-for-capability-contract"],
            &json!({"state":"unknown"}),
        );
        assert_eq!(capability["modes"][0]["launch_runtime"], "native");
        assert!(capability["modes"][0]["default_executable"].is_null());
        assert!(
            capability["modes"][0]["command_requirement"]
                .as_str()
                .unwrap()
                .contains("target executable")
        );
    }

    #[test]
    #[cfg(unix)]
    fn python_cpu_mode_has_independent_prerequisites_from_vmprof() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let interpreter = directory.path().join("selected-python");
        std::fs::write(
            &interpreter,
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'Python 3.12.0\\n'; exit 0; fi\ncase \"$2\" in *vmprof*) exit 1;; *) printf 'CPU counter prerequisite verified\\n';; esac\n",
        )
        .unwrap();
        std::fs::set_permissions(&interpreter, std::fs::Permissions::from_mode(0o755)).unwrap();
        let capability = runtime(
            "cpython",
            &[interpreter.to_str().unwrap()],
            &json!({"state":"unknown"}),
        );
        let modes = capability["modes"].as_array().unwrap();
        assert_eq!(
            modes[0]["availability"],
            "dependency_missing_or_incompatible"
        );
        let cpu = modes
            .iter()
            .find(|mode| mode["mode"] == "python-cpu")
            .unwrap();
        assert_eq!(cpu["launch_runtime"], "python-cpu");
        assert_eq!(cpu["mechanism"], "os-thread-cpu-progress");
        assert_eq!(cpu["availability"], "prerequisites_ready");
        assert_eq!(cpu["default_executable"], interpreter.to_str().unwrap());
        assert_eq!(cpu["selectable"], true);
        assert_eq!(cpu["recording_verified"], false);
        assert!(
            capability["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == "cpu-sampling-prerequisites")
        );
    }
}

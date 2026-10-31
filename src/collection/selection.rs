use anyhow::{Result, bail};

/// The host adapter that prepares a capture, independent of the target language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureAdapter {
    JavaScript,
    PythonVmprof,
    PythonCpuProgress,
    GraalCpuSampler,
    GoTestCpu,
    GoTrace,
    Native,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeRegistration {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub adapter: CaptureAdapter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchExecutableKind {
    RuntimeInterpreter,
    GoTool,
    TargetExecutable,
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureModeRegistration {
    pub mode: &'static str,
    pub launch: &'static RuntimeRegistration,
    pub command_requirement: &'static str,
    pub executable_kind: LaunchExecutableKind,
}

impl RuntimeRegistration {
    fn capability_family(&self) -> &'static str {
        match self.adapter {
            CaptureAdapter::GoTestCpu | CaptureAdapter::GoTrace => "go",
            _ => self.id,
        }
    }

    fn appears_in_capability_family(&self, family: &str) -> bool {
        self.capability_family() == family
            || (!cfg!(windows) && self.id == "python-cpu" && matches!(family, "cpython" | "pypy"))
    }

    fn launch_mode(&'static self) -> CaptureModeRegistration {
        let (mode, command_requirement, executable_kind) = match self.adapter {
            CaptureAdapter::GoTestCpu | CaptureAdapter::GoTrace => (
                self.id,
                "Use go test for one package, followed by test flags.",
                LaunchExecutableKind::GoTool,
            ),
            CaptureAdapter::Native => (
                "automatic-cpu",
                "Pass the built target executable and its arguments.",
                LaunchExecutableKind::TargetExecutable,
            ),
            CaptureAdapter::JavaScript => (
                "automatic-cpu",
                "Pass the selected runtime executable and its script, eval or test arguments.",
                LaunchExecutableKind::RuntimeInterpreter,
            ),
            CaptureAdapter::PythonVmprof
            | CaptureAdapter::PythonCpuProgress
            | CaptureAdapter::GraalCpuSampler => (
                if self.id == "python-cpu" {
                    "python-cpu"
                } else {
                    "automatic-cpu"
                },
                "Pass the selected Python interpreter with a script, -m module or -c code.",
                LaunchExecutableKind::RuntimeInterpreter,
            ),
        };
        CaptureModeRegistration {
            mode,
            launch: self,
            command_requirement,
            executable_kind,
        }
    }
}

const PYTHON_ADAPTER: CaptureAdapter = if cfg!(windows) {
    CaptureAdapter::PythonCpuProgress
} else {
    CaptureAdapter::PythonVmprof
};

const RUNTIMES: &[RuntimeRegistration] = &[
    RuntimeRegistration {
        id: "node",
        aliases: &["nodejs", "javascript", "typescript"],
        adapter: CaptureAdapter::JavaScript,
    },
    RuntimeRegistration {
        id: "bun",
        aliases: &[],
        adapter: CaptureAdapter::JavaScript,
    },
    RuntimeRegistration {
        id: "deno",
        aliases: &[],
        adapter: CaptureAdapter::JavaScript,
    },
    RuntimeRegistration {
        id: "cpython",
        aliases: &["python", "python3"],
        adapter: PYTHON_ADAPTER,
    },
    RuntimeRegistration {
        id: "pypy",
        aliases: &["pypy3"],
        adapter: PYTHON_ADAPTER,
    },
    RuntimeRegistration {
        id: "python-cpu",
        aliases: &[],
        adapter: CaptureAdapter::PythonCpuProgress,
    },
    RuntimeRegistration {
        id: "graalpy",
        aliases: &["graalpython"],
        adapter: CaptureAdapter::GraalCpuSampler,
    },
    RuntimeRegistration {
        id: "go-test",
        aliases: &[],
        adapter: CaptureAdapter::GoTestCpu,
    },
    RuntimeRegistration {
        id: "go-trace",
        aliases: &[],
        adapter: CaptureAdapter::GoTrace,
    },
    RuntimeRegistration {
        id: "go",
        aliases: &[],
        adapter: CaptureAdapter::Native,
    },
    RuntimeRegistration {
        id: "rust",
        aliases: &[],
        adapter: CaptureAdapter::Native,
    },
    RuntimeRegistration {
        id: "c",
        aliases: &[],
        adapter: CaptureAdapter::Native,
    },
    RuntimeRegistration {
        id: "cpp",
        aliases: &["c++"],
        adapter: CaptureAdapter::Native,
    },
    RuntimeRegistration {
        id: "native",
        aliases: &[],
        adapter: CaptureAdapter::Native,
    },
];

/// Registrations describe routing, not verified availability on the current host.
pub fn registered_runtimes() -> &'static [RuntimeRegistration] {
    RUNTIMES
}

pub fn select_runtime(name: &str) -> Result<&'static RuntimeRegistration> {
    if let Some(registration) = RUNTIMES.iter().find(|registration| {
        registration.id.eq_ignore_ascii_case(name)
            || registration
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(name))
    }) {
        return Ok(registration);
    }
    bail!(
        "unsupported runtime {name}; choose {}",
        RUNTIMES
            .iter()
            .map(|registration| registration.id)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Launch choices share the actual route registrations; prerequisite discovery is separate.
pub fn capture_modes(runtime: &str) -> Result<Vec<CaptureModeRegistration>> {
    let family = select_runtime(runtime)?.capability_family();
    let mut modes = RUNTIMES
        .iter()
        .filter(|registration| registration.appears_in_capability_family(family))
        .map(RuntimeRegistration::launch_mode)
        .collect::<Vec<_>>();
    if family == "node" {
        modes.push(CaptureModeRegistration {
            mode: "code-origin",
            launch: select_runtime("node")?,
            command_requirement: "Pass the original Node command and explicitly select code-origin capture. Compiler export fidelity is checked from retained runtime artifacts.",
            executable_kind: LaunchExecutableKind::RuntimeInterpreter,
        });
    }
    modes.sort_by_key(|mode| mode.mode != "automatic-cpu");
    Ok(modes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_and_case_select_one_canonical_runtime_without_changing_mechanism() {
        for (name, canonical, adapter) in [
            ("NODEJS", "node", CaptureAdapter::JavaScript),
            ("JavaScript", "node", CaptureAdapter::JavaScript),
            ("TypeScript", "node", CaptureAdapter::JavaScript),
            ("BUN", "bun", CaptureAdapter::JavaScript),
            ("Deno", "deno", CaptureAdapter::JavaScript),
            ("Python3", "cpython", PYTHON_ADAPTER),
            ("PyPy3", "pypy", PYTHON_ADAPTER),
            ("GraalPython", "graalpy", CaptureAdapter::GraalCpuSampler),
            ("C++", "cpp", CaptureAdapter::Native),
        ] {
            let selected = select_runtime(name).unwrap();
            assert_eq!(selected.id, canonical, "{name}");
            assert_eq!(selected.adapter, adapter, "{name}");
        }
    }

    #[test]
    fn go_native_cpu_and_activated_test_modes_remain_distinct() {
        assert_eq!(
            select_runtime("go").unwrap().adapter,
            CaptureAdapter::Native
        );
        assert_eq!(
            select_runtime("go-test").unwrap().adapter,
            CaptureAdapter::GoTestCpu
        );
        assert_eq!(
            select_runtime("go-trace").unwrap().adapter,
            CaptureAdapter::GoTrace
        );
        assert!(select_runtime("go-attach").is_err());
    }

    #[test]
    fn registrations_cannot_shadow_another_runtime_or_alias() {
        let mut names = std::collections::BTreeSet::new();
        for registration in registered_runtimes() {
            for name in std::iter::once(&registration.id).chain(registration.aliases.iter()) {
                assert!(
                    names.insert(name.to_ascii_lowercase()),
                    "duplicate runtime {name}"
                );
                assert_eq!(select_runtime(name).unwrap().id, registration.id);
            }
        }
        assert!(select_runtime("").is_err());
        assert!(
            select_runtime("none").is_err(),
            "disabled profiling is an execution policy"
        );
    }

    #[test]
    fn capability_launch_modes_resolve_through_the_same_canonical_routes() {
        let modes = capture_modes("GO").unwrap();
        assert_eq!(
            modes
                .iter()
                .map(|mode| (mode.mode, mode.launch.id))
                .collect::<Vec<_>>(),
            [
                ("automatic-cpu", "go"),
                ("go-test", "go-test"),
                ("go-trace", "go-trace"),
            ]
        );
        assert_eq!(
            modes[0].executable_kind,
            LaunchExecutableKind::TargetExecutable
        );
        assert_eq!(modes[1].executable_kind, LaunchExecutableKind::GoTool);
        for registration in registered_runtimes() {
            for mode in capture_modes(registration.id).unwrap() {
                assert!(std::ptr::eq(
                    mode.launch,
                    select_runtime(mode.launch.id).unwrap()
                ));
            }
        }
        assert_eq!(capture_modes("TypeScript").unwrap()[0].launch.id, "node");
        assert!(capture_modes("not-a-runtime").is_err());
    }

    #[test]
    fn python_families_keep_the_explicit_cpu_progress_route() {
        for (family, canonical) in [("Python3", "cpython"), ("PyPy3", "pypy")] {
            let modes = capture_modes(family).unwrap();
            assert_eq!(modes[0].mode, "automatic-cpu");
            assert_eq!(modes[0].launch.id, canonical);
            if cfg!(windows) {
                assert_eq!(modes.len(), 1, "the default already uses CPU progress");
                assert_eq!(modes[0].launch.adapter, CaptureAdapter::PythonCpuProgress);
            } else {
                assert_eq!(modes.len(), 2);
                assert_eq!(modes[1].mode, "python-cpu");
                assert_eq!(modes[1].launch.id, "python-cpu");
                assert_eq!(modes[1].launch.adapter, CaptureAdapter::PythonCpuProgress);
                assert_eq!(
                    modes[1].executable_kind,
                    LaunchExecutableKind::RuntimeInterpreter
                );
            }
        }
    }
}

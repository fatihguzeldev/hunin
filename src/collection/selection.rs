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
}

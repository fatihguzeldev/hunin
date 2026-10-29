use anyhow::{Result, bail};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProfileFormat {
    Firefox,
    V8,
    Graal,
    Pprof,
    PythonCpuProgress,
    GoTrace,
    Vmprof,
    Jsonl,
}

#[derive(Serialize)]
pub(crate) struct ImportFormat {
    #[serde(skip)]
    kind: ProfileFormat,
    format: &'static str,
    label: &'static str,
    description: &'static str,
    aliases: &'static [&'static str],
    requires: &'static [&'static str],
    requires_existing_recording: bool,
}

const FORMATS: &[ImportFormat] = &[
    ImportFormat {
        kind: ProfileFormat::Firefox,
        format: "firefox",
        label: "Native Firefox / Samply profile",
        description: "Native profile with supported symbol and lifetime sidecars. Mapping gaps and unverified source remain explicit.",
        aliases: &["firefox-profile", "samply"],
        requires: &[],
        requires_existing_recording: false,
    },
    ImportFormat {
        kind: ProfileFormat::V8,
        format: "v8-cpuprofile",
        label: "JavaScript CPU profile",
        description: "Node, Bun or Deno CPU profile with supported clock, script and source-map evidence.",
        aliases: &["v8", "cpuprofile"],
        requires: &[],
        requires_existing_recording: false,
    },
    ImportFormat {
        kind: ProfileFormat::Graal,
        format: "graal-cpusampler",
        label: "GraalPy CPU sampler profile",
        description: "Graal CPU sampler JSON and its retained capture metadata.",
        aliases: &["graal"],
        requires: &[],
        requires_existing_recording: false,
    },
    ImportFormat {
        kind: ProfileFormat::Pprof,
        format: "pprof",
        label: "pprof samples",
        description: "Protobuf profile samples with their declared sample types and units.",
        aliases: &["go-pprof"],
        requires: &[],
        requires_existing_recording: false,
    },
    ImportFormat {
        kind: ProfileFormat::PythonCpuProgress,
        format: "python-cpu-progress",
        label: "Python CPU progress recording",
        description: "Bounded stack snapshots attributed to observed OS thread CPU-counter progress.",
        aliases: &[],
        requires: &[],
        requires_existing_recording: false,
    },
    ImportFormat {
        kind: ProfileFormat::GoTrace,
        format: "go-trace",
        label: "Go execution trace",
        description: "Activated Go runtime events and ranges; an execution trace does not imply CPU samples.",
        aliases: &[],
        requires: &["Compatible Go decoder toolchain"],
        requires_existing_recording: false,
    },
    ImportFormat {
        kind: ProfileFormat::Vmprof,
        format: "vmprof",
        label: "CPython / PyPy vmprof recording",
        description: "Original vmprof recording with its supported metadata; unaligned clocks remain unaligned.",
        aliases: &[],
        requires: &["Python decoder environment with the vmprof package"],
        requires_existing_recording: false,
    },
    ImportFormat {
        kind: ProfileFormat::Jsonl,
        format: "hunin-jsonl",
        label: "Hunin observations (JSONL)",
        description: "Observation ownership and references are validated. Retained run IDs are never silently rewritten.",
        aliases: &["jsonl"],
        requires: &["Existing recording identity matching every observation run_id"],
        requires_existing_recording: true,
    },
];

/// Supported decoder inputs, not proof that external decoder prerequisites exist.
pub(crate) fn import_formats() -> &'static [ImportFormat] {
    FORMATS
}

impl ProfileFormat {
    pub(super) fn parse(value: &str) -> Result<Self> {
        if let Some(definition) = FORMATS
            .iter()
            .find(|definition| definition.format == value || definition.aliases.contains(&value))
        {
            return Ok(definition.kind);
        }
        bail!(
            "unsupported profile format {value}; expected {}",
            FORMATS
                .iter()
                .map(|definition| definition.format)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn advertised_canonical_formats_and_aliases_resolve_unambiguously() {
        let mut names = HashSet::new();
        for format in import_formats() {
            for name in std::iter::once(&format.format).chain(format.aliases) {
                assert!(names.insert(name));
                assert_eq!(ProfileFormat::parse(name).unwrap(), format.kind);
            }
        }
        assert!(ProfileFormat::parse("unknown-format").is_err());
    }
}

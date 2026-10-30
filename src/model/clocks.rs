//! Observation time interpretation shared by acceptance and every time view.
//! Mapping labels declare provenance; validation cannot prove producer truth.
use anyhow::{Result, ensure};
use serde_json::Value;

pub(crate) struct Clock<'a> {
    pub aligned: bool,
    pub domain: &'a Value,
    pub mapping: &'a str,
}

fn epoch_mapping(mapping: &str) -> bool {
    matches!(
        mapping,
        "direct_epoch"
            | "aligned"
            | "runtime_anchor"
            | "calibrated_epoch"
            | "collector_epoch_ms"
            | "go_clock_snapshot"
            | "profile_collection_epoch"
            | "profile_start_epoch_plus_relative_time"
            | "os_wait_wall_clock_observation"
    )
}
fn legacy_epoch_source(domain: &str) -> bool {
    matches!(
        domain,
        "time.time_ns"
            | "time.time_ns/perf_counter_ns"
            | "performance.timeOrigin+performance.now"
            | "SystemTime epoch with Instant duration"
            | "time.Time Unix epoch with monotonic duration"
            | "epoch clock with monotonic duration"
    )
}
/// Existing v1 records may omit metadata or carry opaque extensions. They stay
/// readable, immutable and outside shared epoch windows unless explicit known
/// mapping or a concrete legacy epoch source establishes their interpretation.
pub(crate) fn inspect(attributes: &Value) -> Clock<'_> {
    let domain = &attributes["clock"];
    let mapping = attributes["clock_alignment"].as_str();
    let legacy = attributes.get("clock_alignment").is_none()
        && domain.as_str().is_some_and(legacy_epoch_source);
    Clock {
        aligned: legacy || (domain == "unix_epoch" && mapping.is_some_and(epoch_mapping)),
        domain,
        mapping: if legacy {
            "legacy_epoch_source"
        } else {
            mapping.unwrap_or("unknown")
        },
    }
}
/// Unknown labels are retained as unknown; a recognized epoch declaration must
/// have a consistent domain. Historical rows are inspected, not revalidated.
pub(crate) fn validate(attributes: &Value) -> Result<()> {
    if let Some(value) = attributes.get("clock_alignment") {
        let mapping = value
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("observation clock_alignment must be a string"))?;
        ensure!(
            !epoch_mapping(mapping) || attributes["clock"] == "unix_epoch",
            "observation clock_alignment {mapping} requires clock unix_epoch"
        );
    }
    Ok(())
}

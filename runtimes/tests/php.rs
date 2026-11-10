
use hunin_runtimes::{Limits, php};
use serde_json::json;

const PROFILE: &str = "version: 1\ncreator: xdebug 3.5.3 (PHP 8.5.4)\ncmd: fixture.php\npart: 1\npositions: line\nevents: Time_(10ns) Memory_(bytes)\n\nfl=(2) /fixture/a: b.php\nfn=(1) resize\n4 18446744073709551615 8\n\nfl=(2)\nfn=(2) run\n14 30 4\ncfl=(2)\ncfn=(1)\ncalls=7 0 0\n18 120 32\n\nsummary: 150 2048\n";

#[test]
fn xdebug_keeps_exact_self_costs_compressed_names_and_call_counts() {
    let extracted = php::extract(PROFILE.as_bytes(), "php-process", Limits::default()).unwrap();
    assert!(extracted.clocks.is_empty());
    assert!(extracted.warnings.is_empty());
    let cost = extracted
        .records
        .iter()
        .find(|record| {
            record.schema == "xdebug.cost" && record.function.as_deref() == Some("resize")
        })
        .unwrap();
    assert_eq!(cost.payload["time_10ns"], json!(u64::MAX));
    assert_eq!(cost.payload["file"], "/fixture/a: b.php");
    assert_eq!(cost.location.as_deref(), Some("/fixture/a: b.php:4"));
    assert!(cost.time.is_none());
    let call = extracted
        .records
        .iter()
        .find(|record| record.schema == "xdebug.call")
        .unwrap();
    assert_eq!(call.payload["calls"], 7);
    assert_eq!(call.payload["caller_function"], "run");
    assert_eq!(call.payload["callee_function"], "resize");
    assert_eq!(call.payload["target_positions"], json!([0, 0]));
    assert_eq!(call.payload["time_10ns"], 120);
    let caller = extracted
        .records
        .iter()
        .find(|record| record.id == call.payload["caller_cost"])
        .unwrap();
    assert_eq!(caller.function.as_deref(), Some("run"));
    let header = extracted
        .records
        .iter()
        .find(|record| record.schema == "xdebug.profile")
        .unwrap();
    assert_eq!(header.payload["summary"], json!([150, 2048]));
    assert_eq!(header.payload["complete"], true);
    assert_eq!(
        extracted
            .schemas
            .iter()
            .find(|schema| schema.name == "xdebug.cost")
            .unwrap()
            .fields["time_10ns"]
            .unit
            .as_deref(),
        Some("10ns")
    );
}

#[test]
fn xdebug_truncation_keeps_a_marked_prefix_and_incompatible_units_fail() {
    let prefix = PROFILE.split("summary:").next().unwrap();
    let extracted = php::extract(prefix.as_bytes(), "php", Limits::default()).unwrap();
    assert!(!extracted.warnings.is_empty());
    assert!(
        extracted
            .records
            .iter()
            .any(|record| record.schema == "xdebug.cost")
    );
    assert_eq!(extracted.records.last().unwrap().payload["complete"], false);
    for wrong in [
        PROFILE.replace("Time_(10ns)", "Time_(seconds)"),
        PROFILE.replace("fl=(2) /fixture/a: b.php", "fl=(2)"),
        PROFILE.replace("fn=(2) run", "fn=(1) different"),
        PROFILE.replace("positions: line", "positions: instr line"),
        PROFILE.replace("3.5.3", "3.5.4"),
        PROFILE.replace("calls=7 0 0", "calls=18446744073709551616 0 0"),
    ] {
        assert!(php::extract(wrong.as_bytes(), "php", Limits::default()).is_err());
    }
    let pending = format!("{}cfl=(2)\ncfn=(1)\ncalls=1 0 0\n", prefix);
    let extraction = php::extract(pending.as_bytes(), "php", Limits::default()).unwrap();
    assert_eq!(
        extraction.records.last().unwrap().payload["complete"],
        false
    );
}

#[test]
fn xdebug_rejects_input_output_record_and_string_pressure() {
    for limits in [
        Limits {
            max_input_bytes: 8,
            ..Limits::default()
        },
        Limits {
            max_records: 1,
            ..Limits::default()
        },
        Limits {
            max_output_bytes: 8,
            ..Limits::default()
        },
        Limits {
            max_string_bytes: 8,
            ..Limits::default()
        },
    ] {
        assert!(php::extract(PROFILE.as_bytes(), "php", limits).is_err());
    }
}


use hunin::importers;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

fn metadata() -> Value {
    json!({"kind":"sampling","event":"sample_stack","pid":4660,"tid":8384,
        "time_ms":10.0,"raw_timestamp":"9007199254740993","instruction_pointer":"0x6da960",
        "sample_class":"on_cpu","stack_capture_basis":"sampled_instruction_pointer_only",
        "user_stack_depth":0,"kernel_stack_available":false,"reason":"matching user stack unavailable"})
}

fn fixture(events: Vec<Value>, duplicate_thread: bool) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cpu.json");
    let thread = json!({"pid":"4660.1","tid":"8384.2","name":"fixture",
        "samples":{"stack":[0],"time":[10.0],"weight":[1],"threadCPUDelta":[1000]},
        "stackTable":{"frame":[0],"prefix":[null]},"frameTable":{"func":[0],"address":[7186784]},
        "funcTable":{"name":[0],"resource":[-1]},"stringArray":["hot"]});
    let mut threads = vec![thread.clone()];
    if duplicate_thread {
        let mut duplicate = thread;
        duplicate["pid"] = json!("4660.2");
        duplicate["tid"] = json!("8384.3");
        threads.push(duplicate);
    }
    let raw =
        json!({"meta":{"startTime":1700000000000u64,"interval":1.0},"libs":[],"threads":threads});
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(&path, &bytes).unwrap();
    let input = json!({"schema_version":1,"raw_profile_sha256":format!("{:x}",Sha256::digest(&bytes)),
        "epoch_start_ms":1700000000000u64,"truncated":false,
        "collector":{"upstream_commit":"da75c28f367454c621e690eeb4e44ec2ebb29a78","revision":"hunin-native1","os":"windows","patch_sha256":"fixture"},
        "events":events});
    fs::write(
        path.with_extension("lifetimes.json"),
        serde_json::to_vec(&input).unwrap(),
    )
    .unwrap();
    (directory, path)
}

fn sample(records: &[Value]) -> &Value {
    records
        .iter()
        .find(|record| record["kind"] == "sample")
        .unwrap()
}

#[test]
fn original_ip_only_is_explicit_and_keeps_precision_and_reused_id_namespaces() {
    let mapping = json!({"kind":"sample","thread":{"process":{"key":"p0","pid":"4660.1"},"tid":"8384.2"},
        "time_ms":10.0,"mapping_time_ms":10.0,"frames":[{"address":"0x6da960","lookup_address":"0x6da960","mode":"user","load_id":null}]});
    let (_directory, path) = fixture(vec![metadata(), mapping], false);
    let profile = importers::import(&path, "samply", "sampling").unwrap();
    let record = sample(&profile.records);
    assert_eq!(record["pid"], 4660);
    assert_eq!(record["tid"], "8384.2");
    assert_eq!(
        record["attributes"]["sample_stack_basis"],
        "sampled_instruction_pointer_only"
    );
    assert_eq!(record["attributes"]["user_stack_available"], false);
    assert_eq!(record["attributes"]["caller_stack_available"], false);
    assert_eq!(
        record["attributes"]["original_sample_timestamp"],
        "9007199254740993"
    );
    assert_eq!(
        record["attributes"]["sampled_instruction_pointer"],
        "0x6da960"
    );
    assert_eq!(
        record["attributes"]["instruction_pointer_frame_status"],
        "verified"
    );
    assert!(
        profile
            .diagnostics
            .iter()
            .any(|text| text.contains("caller stack"))
    );
    assert!(
        !profile
            .diagnostics
            .iter()
            .any(|text| text.contains("lifetime evidence is incomplete"))
    );
    hunin::ingestion::validate("sampling", &profile.records, &Default::default()).unwrap();
}

#[test]
fn absent_nearby_and_duplicate_sampling_metadata_do_not_acquire_a_stack() {
    for change in ["nearby", "wrong_tid", "duplicate_event", "duplicate_thread"] {
        let mut event = metadata();
        if change == "nearby" {
            event["time_ms"] = json!(10.00000001);
        }
        if change == "wrong_tid" {
            event["tid"] = json!(8385);
        }
        let mut events = vec![event.clone()];
        if change == "duplicate_event" {
            events.push(event);
        }
        let (_directory, path) = fixture(events, change == "duplicate_thread");
        let profile = importers::import(&path, "samply", "sampling").unwrap();
        for record in profile
            .records
            .iter()
            .filter(|record| record["kind"] == "sample")
        {
            assert_eq!(
                record["attributes"]["sample_stack_basis"],
                if change.starts_with("duplicate") {
                    "ambiguous"
                } else {
                    "unavailable"
                },
                "{change}"
            );
            assert!(
                record["attributes"]["caller_stack_available"].is_null(),
                "{change}"
            );
            assert!(
                record["attributes"]["original_sample_timestamp"].is_null(),
                "{change}"
            );
        }
    }
}

#[test]
fn full_user_stack_requires_its_original_event_timestamp_and_retains_emission_time() {
    let mut event = metadata();
    event["stack_capture_basis"] = json!("matched_user_stack");
    event["user_stack_depth"] = json!(2);
    event["stack_event_raw_timestamp"] = event["raw_timestamp"].clone();
    event["stack_emitted_raw_timestamp"] = json!("9007199254741020");
    let (_directory, path) = fixture(vec![event.clone()], false);
    let profile = importers::import(&path, "samply", "sampling").unwrap();
    let record = sample(&profile.records);
    assert_eq!(
        record["attributes"]["sample_stack_basis"],
        "matched_user_stack"
    );
    assert_eq!(record["attributes"]["user_stack_available"], true);
    assert_eq!(record["attributes"]["caller_stack_available"], true);
    assert_eq!(
        record["attributes"]["stack_event_raw_timestamp"],
        "9007199254740993"
    );
    assert_eq!(
        record["attributes"]["stack_emitted_raw_timestamp"],
        "9007199254741020"
    );
    for timestamp in ["9007199254740992", "9007199254740994"] {
        event["stack_event_raw_timestamp"] = json!(timestamp);
        let (_directory, path) = fixture(vec![event.clone()], false);
        assert!(
            importers::import(&path, "samply", "sampling").is_err(),
            "{timestamp}"
        );
    }
}

#[test]
fn fallback_instruction_pointer_cannot_disagree_with_retained_native_frame() {
    let mapping = json!({"kind":"sample","thread":{"process":{"key":"p0","pid":"4660.1"},"tid":"8384.2"},
        "time_ms":10.0,"mapping_time_ms":10.0,"frames":[{"address":"0x6da940","lookup_address":"0x6da940","mode":"user","load_id":null}]});
    let (_directory, path) = fixture(vec![metadata(), mapping], false);
    assert!(importers::import(&path, "samply", "sampling").is_err());
}

#[test]
fn original_kernel_instruction_pointer_is_also_a_partial_single_frame() {
    let mut event = metadata();
    event["instruction_pointer"] = json!("0xfffff80097e8f569");
    event["kernel_stack_available"] = json!(true);
    let mapping = json!({"kind":"sample","thread":{"process":{"key":"p0","pid":"4660.1"},"tid":"8384.2"},
        "time_ms":10.0,"mapping_time_ms":10.0,"frames":[{"address":"0xfffff80097e8f569","lookup_address":"0xfffff80097e8f569","mode":"kernel","load_id":null}]});
    let (_directory, path) = fixture(vec![event, mapping], false);
    let profile = importers::import(&path, "samply", "sampling").unwrap();
    let record = sample(&profile.records);
    assert_eq!(
        record["attributes"]["instruction_pointer_frame_status"],
        "verified"
    );
    assert_eq!(record["attributes"]["caller_stack_available"], false);
}

#[test]
fn legacy_windows_profiles_do_not_claim_verified_caller_stacks() {
    let (_directory, path) = fixture(Vec::new(), false);
    let profile = importers::import(&path, "samply", "sampling").unwrap();
    assert!(
        profile
            .diagnostics
            .iter()
            .any(|text| text.contains("Legacy Windows profile")
                && text.contains("caller chains remain unverified"))
    );
    assert!(sample(&profile.records)["attributes"]["caller_stack_available"].is_null());
    fs::remove_file(path.with_extension("lifetimes.json")).unwrap();
    let mut raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    raw["meta"]["oscpu"] = json!("Windows 10.0.26100");
    fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    let profile = importers::import(&path, "samply", "sampling").unwrap();
    assert!(
        profile
            .diagnostics
            .iter()
            .any(|text| text.contains("Legacy Windows profile"))
    );
    assert!(sample(&profile.records)["attributes"]["caller_stack_available"].is_null());
}

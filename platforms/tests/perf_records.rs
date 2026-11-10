use hunin_platforms::native::perf::{decode_sample, parse_maps};

#[test]
fn perf_samples_keep_integer_time_period_context_and_callchains() {
    let mut payload = Vec::new();
    for value in [
        0x1234_u64,
        (5_u64 << 32) | 42,
        9_007_199_254_740_993,
        2,
        10_101_010,
        2,
        u64::MAX - 511,
        0x1234,
    ] {
        payload.extend(value.to_ne_bytes());
    }
    let sample = decode_sample(&payload, 128).unwrap();
    assert_eq!(sample.process_id, 42);
    assert_eq!(sample.thread_id, 5);
    assert_eq!(sample.timestamp, 9_007_199_254_740_993);
    assert_eq!(sample.weight, 10_101_010);
    assert_eq!(sample.frames[0].resolution, "perf_context_marker");
    assert_eq!(sample.frames[1].address, 0x1234);
    assert!(decode_sample(&payload[..payload.len() - 1], 128).is_err());
    assert!(decode_sample(&payload, 1).is_err());
}

#[test]
fn maps_keep_file_offsets_and_paths_with_spaces() {
    let images = parse_maps("1000-2000 r-xp 00001000 08:01 12 /build/a binary\n2000-3000 rw-p 00002000 08:01 12 /build/a binary\n").unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].file_offset, 4096);
    assert_eq!(images[0].path.to_str(), Some("/build/a binary"));
}

#[test]
fn lifetime_records_preserve_instances_mappings_and_absent_timestamps() {
    use hunin_platforms::native::perf::decode_lifetime;
    let mut mapping = Vec::new();
    mapping.extend(42u32.to_ne_bytes());
    mapping.extend(71u32.to_ne_bytes());
    mapping.extend(0x1000u64.to_ne_bytes());
    mapping.extend(0x2000u64.to_ne_bytes());
    mapping.extend(0x3000u64.to_ne_bytes());
    mapping.extend(b"/build/a module\0");
    let event = decode_lifetime(1, &mapping).unwrap();
    assert!(event["timestamp"].is_null());
    assert_eq!(event["properties"]["pid"], 42);
    assert_eq!(event["properties"]["filename"], "/build/a module");
    assert_eq!(event["properties"]["file_offset"], 0x3000);
    let mut fork = Vec::new();
    for value in [42u32, 10, 71, 11] {
        fork.extend(value.to_ne_bytes());
    }
    fork.extend(9007199254740999u64.to_ne_bytes());
    let event = decode_lifetime(7, &fork).unwrap();
    assert_eq!(event["timestamp"], 9007199254740999u64);
    assert_eq!(event["properties"]["parent_tid"], 11);
    assert!(decode_lifetime(10, &mapping).is_err());
}

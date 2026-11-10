use hunin_platforms::native::{
    Language,
    macos::{parse_atos_location, parse_sample, parse_uuids},
};

#[test]
fn sample_aggregates_preserve_counts_collapsed_addresses_and_image_identity() {
    let text = "Analysis of sampling fixture (pid 42) every 1 millisecond\nCode Type: ARM64\nReport Version: 7\nCall graph:\n    20 Thread_123\n      20 hot_a  (in fixture) + 8,12,...  [0x1008,0x100c,...]  /build/fixture.cpp:17\n        10 leaf  (in fixture) + 4  [0x1014]\n\nTotal number in stack:\nBinary Images:\n  0x1000 - 0x1fff +fixture (0) <12345678-1234-1234-1234-1234567890AB> /build/fixture\n";
    let profile = parse_sample(text, Language::Cpp).unwrap();
    assert!(profile.samples.is_empty());
    assert!(profile.clock.is_none());
    assert_eq!(profile.aggregates.len(), 2);
    assert_eq!(profile.aggregates[0].count, 20);
    assert_eq!(profile.aggregates[0].thread_id, Some(123));
    assert_eq!(profile.aggregates[1].parent, Some(profile.aggregates[0].id));
    assert_eq!(
        profile.aggregates[0].frame.alternate_addresses,
        vec![0x100c]
    );
    assert!(profile.aggregates[0].frame.addresses_truncated);
    assert_eq!(
        profile.aggregates[0].frame.source.as_ref().unwrap().line,
        17
    );
    assert_eq!(
        profile.images[0].identity,
        "12345678-1234-1234-1234-1234567890AB"
    );
}

#[test]
fn uuid_and_source_resolution_do_not_invent_missing_debug_lines() {
    let uuid =
        parse_uuids("UUID: 12345678-1234-1234-1234-1234567890AB (arm64) /tmp/build\n").unwrap();
    assert_eq!(uuid[0].0, "12345678-1234-1234-1234-1234567890AB");
    assert!(parse_uuids("UUID: malformed (arm64) /tmp/build").is_err());
    let location = parse_atos_location("f() (in fixture) (/build/f.cpp:17)").unwrap();
    assert_eq!(
        (location.file.as_str(), location.line),
        ("/build/f.cpp", 17)
    );
    assert!(parse_atos_location("f() (in fixture) + 128").is_none());
}

#[test]
fn unsupported_sample_schema_and_unparsed_code_rows_fail_closed() {
    assert!(
        parse_sample(
            "Report Version: 999\nCall graph:\nBinary Images:\n",
            Language::C
        )
        .is_err()
    );
    assert!(
        parse_sample(
            "Report Version: 7\nCall graph:\n  1 ??? malformed [0x123]\nBinary Images:\n",
            Language::C
        )
        .is_err()
    );
}

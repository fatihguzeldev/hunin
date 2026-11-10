use flate2::{Compression, write::GzEncoder};
use hunin_runtimes::{Limits, pprof};
use std::io::Write;
fn var(mut n: u64) -> Vec<u8> {
    let mut b = vec![];
    while n >= 128 {
        b.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    b.push(n as u8);
    b
}
fn number(tag: u64, n: u64) -> Vec<u8> {
    let mut b = var(tag << 3);
    b.extend(var(n));
    b
}
fn data(tag: u64, v: &[u8]) -> Vec<u8> {
    let mut b = var(tag << 3 | 2);
    b.extend(var(v.len() as u64));
    b.extend(v);
    b
}
fn zip(v: &[u8]) -> Vec<u8> {
    let mut z = GzEncoder::new(vec![], Compression::default());
    z.write_all(v).unwrap();
    z.finish().unwrap()
}
fn profile(location_id: u64) -> Vec<u8> {
    profile_named(location_id, "resize")
}
fn profile_named(location_id: u64, name: &str) -> Vec<u8> {
    let mut root = vec![];
    for text in [
        "",
        "cpu",
        "nanoseconds",
        name,
        "fixture.go",
        "phase",
        "preview",
    ] {
        root.extend(data(6, text.as_bytes()));
    }
    root.extend(data(1, &[number(1, 1), number(2, 2)].concat()));
    root.extend(data(
        5,
        &[number(1, 1), number(2, 3), number(4, 4), number(5, 9)].concat(),
    ));
    let line = [number(1, 1), number(2, 12), number(3, 7)].concat();
    root.extend(data(
        4,
        &[number(1, 1), number(3, 4096), data(4, &line)].concat(),
    ));
    let label = [number(1, 5), number(2, 6)].concat();
    root.extend(data(
        2,
        &[
            data(1, &var(location_id)),
            data(2, &var(30_000_000)),
            data(3, &label),
            data(100, &[0, 255]),
        ]
        .concat(),
    ));
    root.extend(number(10, 100_000_000));
    root.extend(number(12, 10_000_000));
    root.extend(number(101, u64::MAX));
    root
}
#[test]
fn pprof_keeps_units_values_labels_inline_source_and_unknown_wire_extensions() {
    let extraction = pprof::extract(&zip(&profile(1)), "go", Limits::default()).unwrap();
    let header = extraction
        .records
        .iter()
        .find(|r| r.schema == "pprof.profile")
        .unwrap();
    assert_eq!(header.payload["sample_types"][0]["unit"], "nanoseconds");
    assert_eq!(header.payload["extensions"]["101"][0]["unsigned"], u64::MAX);
    assert_eq!(header.payload["string_table"][6], "preview");
    let sample = extraction
        .records
        .iter()
        .find(|r| r.schema == "pprof.sample")
        .unwrap();
    assert_eq!(sample.payload["values"][0], 30_000_000);
    assert_eq!(sample.payload["labels"][0]["string"], "preview");
    assert_eq!(sample.payload["extensions"]["100"][0]["bytes"][1], 255);
    assert_eq!(sample.function.as_deref(), Some("resize"));
    assert_eq!(sample.location.as_deref(), Some("fixture.go:12:7"));
    assert!(sample.time.is_none());
    let query = pprof::costs(&extraction.records, "cpu", "nanoseconds").unwrap();
    assert_eq!(query.functions[0].self_value, 30_000_000);
    assert_eq!(query.functions[0].inclusive_value, 30_000_000);
    assert_eq!(query.unresolved_self_value, 0);
    assert!(pprof::costs(&extraction.records, "cpu", "milliseconds").is_err());
}
#[test]
fn pprof_rejects_broken_references_truncation_trailing_members_and_decompression_budget() {
    assert!(pprof::extract(&zip(&profile(99)), "go", Limits::default()).is_err());
    let mut bad = profile(1);
    bad.extend([0x80]);
    assert!(pprof::extract(&zip(&bad), "go", Limits::default()).is_err());
    let mut trailing = zip(&profile(1));
    trailing.extend(zip(b""));
    assert!(pprof::extract(&trailing, "go", Limits::default()).is_err());
    assert!(
        pprof::extract(
            &zip(&vec![0; 10000]),
            "go",
            Limits {
                max_input_bytes: 100,
                ..Limits::default()
            }
        )
        .is_err()
    );
}

#[test]
fn go_native_lost_sample_markers_make_the_capture_incomplete_without_dropping_weights() {
    let extraction = pprof::extract(
        &zip(&profile_named(1, "runtime/pprof.lostProfileEvent")),
        "go",
        Limits::default(),
    )
    .unwrap();
    assert!(!extraction.capture_complete);
    assert_eq!(extraction.warnings.len(), 1);
    assert!(
        extraction
            .records
            .iter()
            .any(|r| r.schema == "pprof.sample" && r.payload["values"][0] == 30_000_000)
    );
}

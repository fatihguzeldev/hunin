use hunin_platforms::native::{Language, xctrace::parse_time_samples};
#[test]
fn export_keeps_raw_time_thread_stack_weight_and_build_with_references() {
    let xml = r#"<trace-query-result><node><row><sample-time id="1">9007199254740999</sample-time><thread id="2"><tid>71</tid><process><pid>42</pid></process></thread><backtrace id="3"><frame addr="0x1010" name="render"><binary path="/build/app" UUID="12345678-1234-1234-1234-1234567890AB" arch="arm64" load-addr="0x1000"/></frame></backtrace><weight>1000000</weight></row><row><sample-time ref="1"/><thread ref="2"/><backtrace ref="3"/><weight>2000000</weight></row></node></trace-query-result>"#;
    let profile = parse_time_samples(xml, Language::Cpp, "trace-clock").unwrap();
    assert_eq!(profile.samples.len(), 2);
    assert_eq!(profile.samples[1].timestamp, 9007199254740999);
    assert_eq!(profile.samples[1].thread_id, 71);
    assert_eq!(profile.samples[1].weight, 2000000);
    assert_eq!(profile.samples[0].frames[0].address, 0x1010);
    assert_eq!(profile.images.len(), 1);
    assert!(
        parse_time_samples(
            &xml.replace("ref=\"1\"", "ref=\"missing\""),
            Language::C,
            "clock"
        )
        .is_err()
    );
    assert!(
        parse_time_samples(
            &xml.replace("addr=\"0x1010\"", "other=\"0x1010\""),
            Language::C,
            "clock"
        )
        .is_err()
    );
}

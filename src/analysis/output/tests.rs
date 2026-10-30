use super::*;
use std::fs;
use tokio::{sync::watch, time::Instant};

#[test]
fn output_validation_checks_actual_capture_bytes_before_parsing() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("stdout.jsonl");
    fs::File::create(&output)
        .unwrap()
        .set_len(process::OUTPUT_LIMIT + 1)
        .unwrap();
    let (_sender, cancellation) = watch::channel(false);
    let control = process::Control::new(
        cancellation,
        Instant::now() + std::time::Duration::from_secs(30),
    );
    let input = Input {
        bytes: vec![],
        fingerprint: String::new(),
        run_id: "fixture".into(),
        result_ids: vec![],
        record_ids: Default::default(),
    };
    let error = validate(&json!({}), &input, &output, &control).unwrap_err();
    assert!(format!("{error:#}").contains("16 MiB"));
    assert_eq!(
        fs::metadata(output).unwrap().len(),
        process::OUTPUT_LIMIT + 1
    );
}

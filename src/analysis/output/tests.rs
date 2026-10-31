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
        artifacts: Default::default(),
    };
    let error = validate(&json!({}), &input, &output, &control).unwrap_err();
    assert!(format!("{error:#}").contains("16 MiB"));
    assert_eq!(
        fs::metadata(output).unwrap().len(),
        process::OUTPUT_LIMIT + 1
    );
}

#[test]
fn code_proofs_must_match_the_selected_input_artifact_and_its_bounds() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("stdout.jsonl");
    let artifact = crate::model::runs::Artifact {
        id: "source".into(),
        name: "source.bin".into(),
        format: "raw".into(),
        sha256: "a".repeat(64),
        size: 4,
    };
    let input = Input {
        bytes: vec![],
        fingerprint: String::new(),
        run_id: "fixture".into(),
        result_ids: vec![],
        record_ids: ["input".into()].into_iter().collect(),
        artifacts: [(artifact.id.clone(), artifact)].into_iter().collect(),
    };
    let record = json!({"schema_version":1,"kind":"event","id":"unit","run_id":"fixture","producer_id":"mapper","pid":1,"tid":"main","timestamp_us":1,"basis":"derived","evidence":["input"],"name":"code.instance","attributes":{"code_instance":{"schema_version":1,"instance_kind":"static","representation":"source","source":{"path":"source.bin","line":1},"artifact":{"artifact_id":"source","sha256":"a".repeat(64)}}}});
    let (_sender, cancellation) = watch::channel(false);
    let control = process::Control::new(
        cancellation,
        Instant::now() + std::time::Duration::from_secs(30),
    );
    fs::write(&output, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(
        validate(
            &json!({"id":"mapper","package_digest":"digest"}),
            &input,
            &output,
            &control
        )
        .is_ok()
    );
    for proof in [
        json!({"artifact_id":"other-run-artifact","sha256":"a".repeat(64)}),
        json!({"artifact_id":"source","sha256":"b".repeat(64)}),
        json!({"artifact_id":"source","sha256":"a".repeat(64),"offset":3,"length":2}),
    ] {
        let mut invalid = record.clone();
        invalid["attributes"]["code_instance"]["artifact"] = proof;
        fs::write(&output, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(
            validate(
                &json!({"id":"mapper","package_digest":"digest"}),
                &input,
                &output,
                &control
            )
            .is_err()
        );
    }
}

use super::*;
use crate::model::observations::SampleWeightUnit;
use serde_json::json;
use std::collections::HashSet;

#[test]
fn row_and_byte_budgets_are_shared_across_batches_and_fail_sticky() {
    let row = json!({"text":"ünicode\n\""});
    let bytes = serde_json::to_vec(&row).unwrap().len() as u64;
    for limits in [
        Limits {
            rows: 2,
            bytes: 1000,
            ..Default::default()
        },
        Limits {
            bytes: bytes * 2,
            ..Default::default()
        },
    ] {
        let mut budget = OutputBudget::with_limits(limits);
        let mut first = RecordBatch::new(&mut budget, None);
        first.push(row.clone()).unwrap();
        assert_eq!(first.finish().unwrap(), vec![row.clone()]);
        let mut second = RecordBatch::new(&mut budget, None);
        second.push(row.clone()).unwrap();
        assert!(
            second
                .push(row.clone())
                .unwrap_err()
                .downcast_ref::<NormalizationOutputLimit>()
                .is_some()
        );
        assert!(second.finish().is_err());
        assert!(RecordBatch::new(&mut budget, None).push(json!({})).is_err());
    }
}

#[test]
fn oversized_borrowed_frame_is_rejected_before_the_stack_clone() {
    let frame = json!({"function":"large repeated symbol".repeat(10_000)});
    let mut budget = OutputBudget::with_limits(Limits {
        record_bytes: 100,
        ..Default::default()
    });
    let mut batch = RecordBatch::new(&mut budget, None);
    let mut stack = StackBuilder::new();
    let error = stack.push_borrowed(&frame, &mut batch).unwrap_err();
    let error = error.downcast_ref::<NormalizationOutputLimit>().unwrap();
    assert_eq!(error.dimension, OutputDimension::RecordBytes);
    assert_eq!(error.maximum, 100);
    assert_eq!(error.observed, 101);
    assert!(stack.frames.is_empty());
    assert_eq!(batch.len(), 0);
}

#[test]
fn exact_record_frame_and_reserved_relation_limits_preserve_output_order() {
    let frame = json!({"function":"hot"});
    let bytes = serde_json::to_vec(&frame).unwrap().len() as u64;
    let mut budget = OutputBudget::with_limits(Limits {
        record_bytes: bytes,
        frame_visits: 1,
        rows: 2,
        ..Default::default()
    });
    let mut batch = RecordBatch::new(&mut budget, None);
    let relation = batch.reserve(frame.clone()).unwrap();
    batch.push(frame.clone()).unwrap();
    batch.append_reserved(relation).unwrap();
    assert_eq!(batch.finish().unwrap(), vec![frame.clone(), frame]);
    let mut batch = RecordBatch::new(&mut budget, None);
    batch.frame().unwrap();
    let error = batch.frame().unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<NormalizationOutputLimit>()
            .unwrap()
            .dimension,
        OutputDimension::FrameVisits
    );
}

#[test]
fn a_record_is_measured_with_real_json_escaping_at_its_exact_byte_boundary() {
    let row = json!({"text":"çalışma\n\"\\"});
    let bytes = serde_json::to_vec(&row).unwrap().len() as u64;
    for (maximum, accepted) in [(bytes, true), (bytes - 1, false)] {
        let mut budget = OutputBudget::with_limits(Limits {
            record_bytes: maximum,
            ..Default::default()
        });
        let mut records = RecordBatch::new(&mut budget, None);
        let result = records.push(row.clone());
        if accepted {
            result.unwrap();
            assert_eq!(records.finish().unwrap(), vec![row.clone()]);
        } else {
            let error = result.unwrap_err();
            let error = error.downcast_ref::<NormalizationOutputLimit>().unwrap();
            assert_eq!(error.dimension, OutputDimension::RecordBytes);
            assert_eq!(error.observed, maximum + 1);
            assert!(records.records.is_empty());
        }
    }
}

#[test]
fn duplicated_attachment_content_is_charged_without_a_digest_discount() {
    let mut budget = OutputBudget::with_limits(Limits {
        attachment_bytes: 8,
        ..Default::default()
    });
    budget.attachment_content(4).unwrap();
    budget.attachment_content(4).unwrap();
    assert_eq!(budget.attachments, 8);
    let error = budget.attachment_content(1).unwrap_err();
    let error = error.downcast_ref::<NormalizationOutputLimit>().unwrap();
    assert_eq!(error.dimension, OutputDimension::AttachmentBytes);
    assert_eq!(error.maximum, 8);
    assert_eq!(error.observed, 9);
    assert!(budget.attachment_content(0).is_err());
}

#[test]
fn metadata_serialization_counts_real_bytes_and_stops_at_one_overflow_sentinel() {
    let metadata = json!({"extension":"large metadata".repeat(100)});
    let bytes = serde_json::to_vec(&metadata).unwrap().len() as u64;
    let mut exact = OutputBudget::with_limits(Limits {
        attachment_bytes: bytes,
        ..Default::default()
    });
    exact.metadata(&metadata).unwrap();
    assert_eq!(exact.attachments, bytes);
    let mut limited = OutputBudget::with_limits(Limits {
        attachment_bytes: bytes - 1,
        ..Default::default()
    });
    let error = limited.metadata(&metadata).unwrap_err();
    let error = error.downcast_ref::<NormalizationOutputLimit>().unwrap();
    assert_eq!(error.dimension, OutputDimension::AttachmentBytes);
    assert_eq!(error.observed, bytes);
}

#[test]
fn common_profile_gate_counts_each_source_image_and_metadata_copy_across_profiles() {
    let profile = crate::importers::ImportedProfile {
        source: "same/raw.profile".into(),
        raw_sha256: "a".repeat(64),
        format: "control".into(),
        records: vec![],
        diagnostics: vec!["same diagnostic".into()],
        input_fingerprint: "b".repeat(64),
        sidecars: vec![],
        sidecar_sha256: Default::default(),
        source_snapshots: vec![crate::importers::SourceSnapshot {
            uri: "file:///same.ts".into(),
            content: "same source".repeat(30),
            sha256: "c".repeat(64),
            provenance: json!({"extension":"same metadata"}),
        }],
        binary_snapshots: vec![crate::importers::BinarySnapshot {
            sha256: "d".repeat(64),
            content: vec![7; 256],
        }],
        native_image_context: Some(json!({"checks":{"same":"image"}})),
        decoder_provenance: Some(json!({"identity":{"decoder":"control"}})),
    };
    let mut measured = OutputBudget::default();
    measured.profile(&profile).unwrap();
    let one = measured.attachments;
    assert!(
        one > profile.source_snapshots[0].content.len() as u64
            + profile.binary_snapshots[0].content.len() as u64
    );
    let mut budget = OutputBudget::with_limits(Limits {
        attachment_bytes: one * 2,
        ..Default::default()
    });
    budget.profile(&profile).unwrap();
    budget.profile(&profile).unwrap();
    assert_eq!(budget.attachments, one * 2);
    let error = budget.profile(&profile).unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<NormalizationOutputLimit>()
            .unwrap()
            .dimension,
        OutputDimension::AttachmentBytes
    );
}

#[test]
fn canonical_sample_units_emit_the_authoritative_wire_contract_without_losing_attributes() {
    for (unit, wire) in [
        (SampleWeightUnit::Samples, "samples"),
        (SampleWeightUnit::Microseconds, "microseconds"),
        (SampleWeightUnit::Bytes, "bytes"),
        (SampleWeightUnit::Count, "count"),
    ] {
        let attributes = json!({"clock":"process_cpu","clock_alignment":"unmapped","extension":{"retained":[null,"opaque"]}});
        let row = sample(
            "unit-control",
            "input",
            0,
            StackSample {
                pid: 3,
                tid: "thread",
                timestamp_us: 0,
                stack: vec![json!({"function":"measured"})],
                weight: 2.5,
                weight_unit: unit,
                attributes: attributes.clone(),
            },
        );
        crate::ingestion::validate("unit-control", std::slice::from_ref(&row), &HashSet::new())
            .unwrap();
        assert_eq!(row["weight_unit"], wire);
        assert_eq!(row["weight"], 2.5);
        assert_eq!(row["basis"], "sampled");
        assert_eq!(row["attributes"], attributes);
        assert_eq!(crate::query::summary(&[row])["weight_unit"], wire);
    }
}

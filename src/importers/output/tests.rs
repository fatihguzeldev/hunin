use super::*;
use serde_json::json;

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

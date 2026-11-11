use hunin_platforms::native::etw::{profile_event, stack_event, target_stack_event};
#[test]
fn etw_joins_have_original_timestamp_thread_pid_and_pointer_width() {
    let mut raw = Vec::new();
    raw.extend(0x1234567812345678u64.to_le_bytes());
    raw.extend(71u32.to_le_bytes());
    raw.extend(5u16.to_le_bytes());
    raw.extend(0u16.to_le_bytes());
    let sample = profile_event(&raw, 8, 2).unwrap();
    assert_eq!(
        (sample.thread_id, sample.instruction_pointer, sample.count),
        (71, 0x1234567812345678, 5)
    );
    let timestamp = 9_007_199_254_740_999u64;
    let mut stack = Vec::new();
    stack.extend(timestamp.to_le_bytes());
    stack.extend(42u32.to_le_bytes());
    stack.extend(71u32.to_le_bytes());
    stack.extend(0xffff1234u32.to_le_bytes());
    let stack = stack_event(&stack, 4, 2, 128).unwrap();
    assert_eq!(
        (stack.timestamp, stack.process_id, stack.thread_id),
        (timestamp, 42, 71)
    );
    assert_eq!(stack.addresses, vec![0xffff1234]);
    assert!(profile_event(&raw, 4, 2).is_err());
    assert!(stack_event(&[0; 16], 8, 99, 128).is_err());
    assert!(stack_event(&[0; 24], 8, 2, 0).is_err());
}

#[test]
fn foreign_system_stacks_do_not_consume_the_selected_targets_frame_budget() {
    let mut bytes = vec![0; 16 + 192 * 8];
    bytes[8..12].copy_from_slice(&42u32.to_le_bytes());
    assert_eq!(
        target_stack_event(&bytes, 8, 2, 42)
            .unwrap()
            .unwrap()
            .addresses
            .len(),
        192
    );
    bytes.extend([0; 8]);
    assert!(target_stack_event(&bytes, 8, 2, 42).is_err());
    assert!(target_stack_event(&bytes, 8, 2, 43).unwrap().is_none());
    assert!(target_stack_event(&[0; 15], 8, 2, 43).is_err());
}

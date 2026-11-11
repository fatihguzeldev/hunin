use hunin_platforms::process::{
    Counter, ErrorKind, ProcessInstance, inspect_process, read_resources,
};

#[test]
fn mach_and_jiffy_counters_keep_exact_scale_and_check_overflow() {
    let ticks = Counter::new(24_000_000, 125, 3, "mach_ticks").unwrap();
    assert_eq!(ticks.nanoseconds().unwrap(), 1_000_000_000);
    assert_eq!(
        Counter::new(59, 1_000_000_000, 100, "clock_ticks")
            .unwrap()
            .nanoseconds()
            .unwrap(),
        590_000_000
    );
    assert!(Counter::new(1, 1, 0, "invalid").is_err());
    assert!(
        Counter::new(u64::MAX, u64::MAX, 1, "overflow")
            .unwrap()
            .nanoseconds()
            .is_err()
    );
}

#[test]
fn current_process_resources_are_bound_to_a_real_instance() {
    let instance = inspect_process(std::process::id()).unwrap();
    let first = read_resources(&instance).unwrap();
    let mut sum = 0_u64;
    for i in 0..1_000_000 {
        sum = std::hint::black_box(sum.wrapping_add(i));
    }
    std::hint::black_box(sum);
    let second = read_resources(&instance).unwrap();
    assert!(first.instance.same_instance(&instance));
    assert!(second.instance.same_instance(&instance));
    assert!(
        second.user_cpu.nanoseconds().unwrap() + second.system_cpu.nanoseconds().unwrap()
            >= first.user_cpu.nanoseconds().unwrap() + first.system_cpu.nanoseconds().unwrap()
    );
    assert!(second.memory.iter().any(|m| m.bytes > 0));
    assert!(second.acquired.end_ns >= second.acquired.start_ns);
}

#[cfg(windows)]
#[test]
fn caller_supplied_parentage_is_never_published_as_a_resource_observation() {
    let mut expected = inspect_process(std::process::id()).unwrap();
    expected.parent_pid = Some(u32::MAX);
    let snapshot = read_resources(&expected).unwrap();
    assert!(snapshot.instance.same_instance(&expected));
    assert_eq!(snapshot.instance.parent_pid, None);
    let record = snapshot.to_record("sample", "target").unwrap();
    assert_eq!(
        record.payload["instance"]["parent_pid"],
        serde_json::Value::Null
    );
}

#[test]
fn a_stale_identity_never_becomes_a_new_process() {
    let mut instance: ProcessInstance = inspect_process(std::process::id()).unwrap();
    instance.start_identity.push_str("-not-this-instance");
    assert_eq!(
        read_resources(&instance).unwrap_err().kind,
        ErrorKind::InstanceChanged
    );
}

#[test]
fn invalid_resource_target_pid_is_rejected_before_os_inspection() {
    let mut instance = inspect_process(std::process::id()).unwrap();
    for pid in [0, u32::MAX] {
        instance.pid = pid;
        assert_eq!(
            read_resources(&instance).unwrap_err().kind,
            ErrorKind::InvalidInput
        );
    }
}

#[test]
fn corrupt_unit_scale_or_reversed_acquisition_is_rejected_before_publication() {
    let instance = hunin_platforms::inspect_process(std::process::id()).unwrap();
    let original = hunin_platforms::read_resources(&instance).unwrap();
    let mut invalid = original.clone();
    invalid.user_cpu.denominator = 0;
    assert!(invalid.to_record("bad", "target").is_err());
    let mut invalid = original;
    invalid.acquired.end_ns = invalid.acquired.start_ns.saturating_sub(1);
    assert!(invalid.to_record("bad", "target").is_err());
}

#[cfg(unix)]
#[test]
fn actual_process_cpu_units_agree_with_independent_getrusage_endpoints() {
    fn cpu() -> u64 {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
        ((usage.ru_utime.tv_sec as u64 + usage.ru_stime.tv_sec as u64) * 1_000_000
            + usage.ru_utime.tv_usec as u64
            + usage.ru_stime.tv_usec as u64)
            * 1000
    }
    let instance = hunin_platforms::inspect_process(std::process::id()).unwrap();
    let before = hunin_platforms::read_resources(&instance).unwrap();
    let reference_before = cpu();
    let start = std::time::Instant::now();
    let mut value = 1_u64;
    while start.elapsed() < std::time::Duration::from_millis(250) {
        for i in 0..1000 {
            value = std::hint::black_box(value.wrapping_mul(6364136223846793005).wrapping_add(i));
        }
    }
    std::hint::black_box(value);
    let reference = cpu() - reference_before;
    let after = hunin_platforms::read_resources(&instance).unwrap();
    let observed = after.user_cpu.nanoseconds().unwrap() + after.system_cpu.nanoseconds().unwrap()
        - before.user_cpu.nanoseconds().unwrap()
        - before.system_cpu.nanoseconds().unwrap();
    assert!(reference > 100_000_000);
    assert!(
        observed.abs_diff(reference) <= 50_000_000.max(reference / 20),
        "source {observed}ns vs independent {reference}ns; raw scales must remain visible"
    );
}

#[cfg(windows)]
#[test]
fn windows_process_cpu_scale_agrees_with_independent_busy_thread_times() {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentThread, GetThreadTimes},
    };
    fn reference() -> u64 {
        let mut creation: FILETIME = unsafe { std::mem::zeroed() };
        let mut exit = creation;
        let mut kernel = creation;
        let mut user = creation;
        assert_ne!(
            unsafe {
                GetThreadTimes(
                    GetCurrentThread(),
                    &mut creation,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                )
            },
            0
        );
        let ticks =
            |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        (ticks(user) + ticks(kernel)) * 100
    }
    let instance = inspect_process(std::process::id()).unwrap();
    let before = read_resources(&instance).unwrap();
    let thread_before = reference();
    let start = std::time::Instant::now();
    let mut value = 1_u64;
    while start.elapsed() < std::time::Duration::from_millis(250) {
        for i in 0..1000 {
            value = std::hint::black_box(value.wrapping_mul(6364136223846793005).wrapping_add(i));
        }
    }
    std::hint::black_box(value);
    let thread_ns = reference() - thread_before;
    let after = read_resources(&instance).unwrap();
    let observed = after.user_cpu.nanoseconds().unwrap() + after.system_cpu.nanoseconds().unwrap()
        - before.user_cpu.nanoseconds().unwrap()
        - before.system_cpu.nanoseconds().unwrap();
    assert!(thread_ns >= 100_000_000);
    assert!(
        observed.abs_diff(thread_ns) <= 50_000_000,
        "process {observed}ns vs independent busy thread {thread_ns}ns"
    );
    assert_eq!(after.user_cpu.source_unit, "100ns");
    assert_eq!(after.user_cpu.nanoseconds_numerator, 100);
    assert_eq!(after.user_cpu.denominator, 1);
}

#[cfg(windows)]
#[test]
fn windows_private_commit_and_working_set_keep_their_distinct_byte_meanings() {
    let instance = inspect_process(std::process::id()).unwrap();
    let first = read_resources(&instance).unwrap();
    let mut allocation = vec![0u8; 32 * 1024 * 1024];
    for page in allocation.chunks_mut(4096) {
        page[0] = std::hint::black_box(71);
    }
    std::hint::black_box(&allocation);
    let second = read_resources(&instance).unwrap();
    for name in [
        "windows.PROCESS_MEMORY_COUNTERS_EX.WorkingSetSize",
        "windows.PROCESS_MEMORY_COUNTERS_EX.PrivateUsage",
    ] {
        let before = first.memory.iter().find(|m| m.definition == name).unwrap();
        let after = second.memory.iter().find(|m| m.definition == name).unwrap();
        assert!(
            after.bytes >= before.bytes + 28 * 1024 * 1024,
            "{name}: {} to {} bytes",
            before.bytes,
            after.bytes
        );
        assert!(!after.approximate);
    }
}

use hunin_platforms::process::{parse_linux_stat, parse_smaps_rollup};

#[test]
fn stat_parser_handles_parentheses_and_spaces_and_keeps_children_out() {
    let mut fields = vec!["0"; 22];
    fields[0] = "S";
    fields[1] = "9";
    fields[11] = "59";
    fields[12] = "7";
    fields[13] = "999";
    fields[14] = "888";
    fields[19] = "164";
    let stat = parse_linux_stat(&format!("42 (weird ) name (here) {})", fields.join(" ")));
    assert!(stat.is_err());
    let stat = parse_linux_stat(&format!("42 (weird ) name (here) {}", fields.join(" "))).unwrap();
    assert_eq!(
        (
            stat.pid,
            stat.parent_pid,
            stat.user_ticks,
            stat.system_ticks,
            stat.start_ticks
        ),
        (42, 9, 59, 7, 164)
    );
}

#[test]
fn malformed_and_overflowing_procfs_values_are_rejected() {
    assert!(parse_linux_stat("1 (foo) R 2").is_err());
    assert!(
        parse_smaps_rollup(
            "aaaa0000-ffff0000 ---p 00000000 00:00 0 [rollup]\nRss: 18446744073709551615 kB\n"
        )
        .is_err()
    );
    assert!(parse_smaps_rollup("Rss: 123 MB\n").is_err());
    assert!(parse_smaps_rollup("Rss: 123 kB\nRss: 456 kB\n").is_err());
    let readings = parse_smaps_rollup("Rss: 64 kB\nPss: 32 kB\n").unwrap();
    assert_eq!(readings[0].bytes, 65_536);
    assert_eq!(readings[1].bytes, 32_768);
}

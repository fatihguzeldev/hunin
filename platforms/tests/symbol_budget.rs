#![cfg(unix)]
use hunin_platforms::{
    native::{Frame, Image, Language, Profile, Sample, symbols},
    process::ErrorKind,
    tools,
};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, process::Command, time::Duration};

#[test]
fn repeated_elf_symbols_fail_atomically_before_profile_expansion() {
    let area = tempfile::tempdir().unwrap();
    let mut bytes = vec![0u8; 140];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
    bytes[64..68].copy_from_slice(&4u32.to_le_bytes());
    bytes[72..80].copy_from_slice(&120u64.to_le_bytes());
    bytes[96..104].copy_from_slice(&20u64.to_le_bytes());
    bytes[120..124].copy_from_slice(&4u32.to_le_bytes());
    bytes[124..128].copy_from_slice(&4u32.to_le_bytes());
    bytes[128..132].copy_from_slice(&3u32.to_le_bytes());
    bytes[132..136].copy_from_slice(b"GNU\0");
    bytes[136..140].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
    fs::write(area.path().join("image.elf"), bytes).unwrap();
    let tool = area.path().join("llvm-symbolizer");
    // one valid response below the tool's 4 mib output cap, reused by 1100 samples.
    let response = json!({"Symbol":[{"FunctionName":"f".repeat(65536),"FileName":"/fixture.c","Line":7,"Column":1}]}).to_string();
    fs::write(&tool, format!("#!/bin/sh\nprintf '%s\\n' '{response}'\n")).unwrap();
    fs::set_permissions(tool, fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "symbol_budget_fixture",
            "--nocapture",
        ])
        .env("HUNIN_SYMBOL_PRESSURE_AREA", area.path())
        .env(
            "PATH",
            std::env::join_paths(std::iter::once(area.path().to_path_buf()).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ))
            .unwrap(),
        );
    tools::require_success(
        tools::run(&mut command, area.path(), Duration::from_secs(10), 8192).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "finite child process for the synthetic symbolizer pressure control"]
fn symbol_budget_fixture() {
    let area = std::path::PathBuf::from(
        std::env::var_os("HUNIN_SYMBOL_PRESSURE_AREA").expect("pressure fixture area"),
    );
    let image = area.join("image.elf");
    let sample = Sample {
        instruction_pointer: 0x1100,
        producer_fields: json!({}),
        process_id: 1,
        timestamp: 0,
        thread_id: 1,
        cpu: None,
        weight: 1,
        weight_unit: "samples".into(),
        frames: vec![Frame {
            address: 0x1100,
            alternate_addresses: vec![],
            addresses_truncated: false,
            image: Some(0),
            function: None,
            reported_function: None,
            inline_sites: vec![],
            source: None,
            resolution: "unresolved".into(),
        }],
    };
    let mut profile = Profile {
        schema: "synthetic-pressure-control".into(),
        producer: "adapter test, not a real runtime capture".into(),
        language: Language::C,
        images: vec![Image {
            path: image.clone(),
            architecture: "x86_64".into(),
            identity_kind: "elf_build_id".into(),
            identity: "11223344".into(),
            load_address: 0x1000,
            file_offset: 0,
            mapped_virtual_address: None,
            end_address: Some(0x2000),
        }],
        samples: vec![sample; 1100],
        aggregates: vec![],
        clock: None,
        ticks_per_second: 1_000_000_000,
        period: None,
        start: None,
        end: None,
        issues: vec![],
        metadata: json!({}),
    };
    let before = serde_json::to_vec(&profile).unwrap();
    assert!(before.len() < 512 * 1024);
    let error = symbols::resolve_elf_profile(&mut profile, &image, &area).unwrap_err();
    assert_eq!(error.kind, ErrorKind::LimitExceeded);
    assert_eq!(serde_json::to_vec(&profile).unwrap(), before);
    assert!(profile.records("retained-raw").is_ok());
}

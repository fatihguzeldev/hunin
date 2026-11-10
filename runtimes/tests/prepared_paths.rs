#[cfg(unix)]
#[test]
fn prepared_extraction_rejects_symlinks_and_fifos_without_following_or_blocking() {
    use hunin_runtimes::{Limits, RuntimeKind, prepare};
    use std::{ffi::CString, fs, os::unix::fs::symlink, path::PathBuf};
    let directory = tempfile::tempdir().unwrap();
    let workload = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/preview.py");
    let prepared = prepare(
        RuntimeKind::CPython,
        std::path::Path::new("/usr/bin/true"),
        &workload,
        directory.path(),
    )
    .unwrap();
    let other = tempfile::NamedTempFile::new().unwrap();
    fs::write(other.path(), b"{}").unwrap();
    symlink(other.path(), &prepared.profile_path).unwrap();
    assert!(prepared.extract("p", Limits::default()).is_err());
    fs::remove_file(&prepared.profile_path).unwrap();
    let path = CString::new(prepared.profile_path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let start = std::time::Instant::now();
    assert!(prepared.extract("p", Limits::default()).is_err());
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn target_configuration_preserves_collector_environment_and_lifecycle_ownership() {
    use hunin_core::execution::PreparedLaunch;
    use hunin_runtimes::{PreparedRuntime, RuntimeKind};
    use std::{collections::BTreeMap, path::PathBuf};
    let make = || PreparedRuntime {
        kind: RuntimeKind::Node,
        profile_path: PathBuf::from("profile"),
        report_path: PathBuf::from("report"),
        launch: PreparedLaunch {
            program: PathBuf::from("runtime"),
            args: vec![],
            directory: PathBuf::from("area"),
            environment: BTreeMap::from([
                ("TMPDIR".into(), "owned-temp".into()),
                ("XDEBUG_MODE".into(), "profile".into()),
            ]),
            stdout_limit: 1024,
        },
    };
    for (key, value) in [
        ("TMPDIR", "elsewhere"),
        ("XDEBUG_MODE", "off"),
        ("HUNIN_FINALIZE_GATE", "0"),
        ("HUNIN_CONTROL_PROTOCOL", "legacy"),
        ("HUNIN_CONTROL_TOKEN", "caller-supplied"),
        ("hunin_control_token", "caller-supplied"),
    ] {
        let mut prepared = make();
        let before = prepared.launch.environment.clone();
        assert!(
            prepared
                .configure(&BTreeMap::from([(key.into(), value.into())]), &[], &[])
                .is_err()
        );
        assert_eq!(prepared.launch.environment, before);
    }
    let mut prepared = make();
    prepared
        .configure(
            &BTreeMap::from([
                ("PREVIEW_INPUT".into(), "selected-assets".into()),
                ("TMPDIR".into(), "owned-temp".into()),
            ]),
            &[],
            &[],
        )
        .unwrap();
    assert_eq!(
        prepared.launch.environment["PREVIEW_INPUT"],
        "selected-assets"
    );
    assert_eq!(prepared.launch.environment["TMPDIR"], "owned-temp");
    #[cfg(windows)]
    {
        let mut prepared = make();
        assert!(
            prepared
                .configure(
                    &BTreeMap::from([("tmpdir".into(), "elsewhere".into())]),
                    &[],
                    &[]
                )
                .is_err()
        );
    }
}

#[cfg(unix)]
#[test]
fn interpreted_preparation_requests_factory_owned_nonce_control() {
    use hunin_runtimes::{RuntimeKind, prepare};
    use std::path::{Path, PathBuf};
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for (kind, fixture) in [
        (RuntimeKind::Node, "preview.cjs"),
        (RuntimeKind::Deno, "preview.cjs"),
        (RuntimeKind::Bun, "preview.cjs"),
        (RuntimeKind::CPython, "preview.py"),
        (RuntimeKind::PyPy, "preview.py"),
        (RuntimeKind::CRuby, "preview.rb"),
    ] {
        let area = tempfile::tempdir().unwrap();
        let prepared = prepare(
            kind,
            Path::new("/usr/bin/true"),
            &fixtures.join(fixture),
            area.path(),
        )
        .unwrap();
        assert_eq!(
            prepared.launch.environment["HUNIN_CONTROL_PROTOCOL"],
            "nonce-lines-v1"
        );
        assert!(
            !prepared
                .launch
                .environment
                .contains_key("HUNIN_CONTROL_TOKEN")
        );
        if kind == RuntimeKind::Deno {
            assert!(
                prepared
                    .launch
                    .args
                    .iter()
                    .any(|arg| arg.starts_with("--allow-env=")
                        && arg.contains("HUNIN_CONTROL_TOKEN"))
            );
        }
    }
}

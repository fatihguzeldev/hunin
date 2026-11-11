use std::{path::PathBuf, process::Command, time::Duration};
#[test]
#[ignore = "requires selected Node for browser helper lifecycle controls; no browser is launched"]
fn firefox_close_releases_owned_driver_pipes_even_when_session_shutdown_fails() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let area = tempfile::tempdir().unwrap();
    let output = hunin_platforms::tools::run(
        Command::new(std::env::var_os("HUNIN_NODE").expect("selected Node"))
            .arg(root.join("tests/fixtures/browser-close-control.cjs"))
            .arg(root.join("helpers/browser.cjs"))
            .arg(area.path()),
        area.path(),
        Duration::from_secs(5),
        65536,
    )
    .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
}

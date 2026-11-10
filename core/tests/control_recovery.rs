use hunin_core::control::{Action, Controller, RequestId};
use hunin_core::project::Project;
use serde_json::json;
use std::io::Write;

fn prepare(
    path: &std::path::Path,
) -> (
    Project,
    Controller,
    hunin_core::project::Reference,
    RequestId,
) {
    let p = Project::create(path, "crash-project").unwrap();
    let i = p
        .create_investigation("i", "crash recovery", json!({}))
        .unwrap();
    let r = p.create_run(&i, "r", json!({})).unwrap();
    let key = RequestId {
        project: p.id().into(),
        caller: "agent".into(),
        id: "known-before-launch".into(),
    };
    (p.clone(), Controller::new(p), r, key)
}

#[test]
fn crash_worker() {
    let Some(root) = std::env::var_os("HUNIN_CRASH_TEST_ROOT") else {
        return;
    };
    let point = std::env::var("HUNIN_CRASH_TEST_POINT").unwrap();
    let (p, c, r, key) = prepare(std::path::Path::new(&root));
    if point == "before_intent" {
        std::process::exit(77)
    }
    c.accept(&key, &r, Action::Launch, json!({"program":"fixture"}))
        .unwrap();
    if point == "after_intent" {
        std::process::exit(77)
    }
    let dispatched = c.dispatch(&key).unwrap();
    if point == "after_dispatch" {
        std::process::exit(77)
    }
    let mut effect = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::Path::new(&root).join("independent-effect-ledger"))
        .unwrap();
    effect.write_all(b"effect\n").unwrap();
    effect.sync_all().unwrap();
    if point == "after_effect" {
        std::process::exit(77)
    }
    c.complete(
        &key,
        dispatched.generation,
        true,
        json!({"effect":"observed"}),
    )
    .unwrap();
    drop(p);
    std::process::exit(77)
}

#[test]
fn interruption_at_every_effect_barrier_never_redispatches_an_accepted_request() {
    for point in [
        "before_intent",
        "after_intent",
        "after_dispatch",
        "after_effect",
        "after_result",
    ] {
        let root = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_worker", "--nocapture"])
            .env("HUNIN_CRASH_TEST_ROOT", root.path())
            .env("HUNIN_CRASH_TEST_POINT", point)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(77), "{point}");
        let (_p, c, r, key) = prepare(root.path());
        let accepted = c
            .accept(&key, &r, Action::Launch, json!({"program":"fixture"}))
            .unwrap();
        assert_eq!(accepted.newly_accepted, point == "before_intent", "{point}");
        assert_eq!(
            c.lookup(&key).unwrap().arguments,
            json!({"program":"fixture"})
        );
        if matches!(point, "after_dispatch" | "after_effect") {
            assert!(c.dispatch(&key).is_err(), "{point}");
            assert_eq!(c.interrupted(&key).unwrap().state, "uncertain");
        }
        if point == "after_result" {
            assert_eq!(c.lookup(&key).unwrap().state, "succeeded");
        }
        let effects = std::fs::read_to_string(root.path().join("independent-effect-ledger"))
            .unwrap_or_default();
        assert_eq!(
            effects.lines().count(),
            usize::from(matches!(point, "after_effect" | "after_result")),
            "{point}"
        );
    }
}

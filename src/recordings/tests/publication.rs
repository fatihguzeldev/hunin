use super::*;
use crate::recordings::publication::{publish_controlled, publish_with_limits};

#[test]
fn changed_raw_or_sidecar_never_publishes_a_mismatched_projection() {
    for changed in ["cpu.json", "cpu.syms.json"] {
        let input = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        for name in ["cpu.json", "cpu.syms.json"] {
            fs::copy(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/importers/fixtures/native-reload")
                    .join(name),
                input.path().join(name),
            )
            .unwrap();
        }
        let store = Store::open(data.path()).unwrap();
        let run = serde_json::from_value(json!({"id":"stable-inputs","name":"control","status":"completed","command":[],"cwd":"/removed","runtime":"native","started_at_us":1,"finished_at_us":2,"exit_code":0,"diagnostics":[],"source":{},"correctness":{}})).unwrap();
        store.save_run(&run).unwrap();
        let profile =
            crate::importers::import(&input.path().join("cpu.json"), "firefox", "stable-inputs")
                .unwrap();
        fs::write(input.path().join(changed), b"replaced after parsing").unwrap();
        let error = publish_controlled(&store, "stable-inputs", profile, None)
            .err()
            .unwrap();
        assert!(
            error.to_string().contains("changed during normalization"),
            "{error:#}"
        );
        assert!(store.records("stable-inputs").unwrap().is_empty());
    }
}
#[test]
fn publication_rehash_and_expired_deadline_never_accept_a_valid_prefix() {
    let (_root, store, path) = fixture();
    for deadline in [None, Some(std::time::Instant::now())] {
        let profile = crate::importers::import(&path, "jsonl", "bounded-import").unwrap();
        let mut limits = crate::importers::NormalizationInputLimits::default();
        if deadline.is_none() {
            limits.raw_bytes = fs::metadata(&path).unwrap().len() - 1;
        }
        assert!(publish_with_limits(&store, "bounded-import", profile, limits, deadline).is_err());
        assert!(store.records("bounded-import").unwrap().is_empty());
        assert_eq!(store.documents("profile-inputs").unwrap().len(), 0);
    }
    assert!(path.is_file());
}

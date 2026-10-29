use super::{CapturePlan, CaptureRequest};
use serde_json::{Value, json};
use std::collections::BTreeMap;

impl CapturePlan {
    /// A comparison view of the plan; actual launch arguments and identities stay intact.
    pub fn settings(&self, request: &CaptureRequest) -> Value {
        let normalize = |value: &str| {
            let mut normalized = value
                .replace(
                    request.artifact_dir.to_string_lossy().as_ref(),
                    "<artifacts>",
                )
                .replace(&request.run_id, "<run>");
            for (role, identity) in &self.session_identity {
                if !identity.is_empty() {
                    normalized = normalized.replace(identity, &format!("<{role}>"));
                }
            }
            normalized
        };
        let mut settings = json!({
            "runtime": request.runtime,
            "runtime_version": self.runtime_version,
            "command": self.command.iter().map(|value| normalize(value)).collect::<Vec<_>>(),
            "environment": self.env.iter().map(|(key, value)| (key.clone(), normalize(value))).collect::<BTreeMap<_, _>>(),
        });
        if let Some(identity) = &self.collector_identity {
            settings["collector_identity"] = identity.clone();
        }
        settings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_session_tokens_preserve_deno_marker_without_mutating_launch_values() {
        let request = CaptureRequest {
            command: vec!["deno".into(), "run".into(), "main.ts".into()],
            cwd: "/project".into(),
            artifact_dir: "/capture/recording-one".into(),
            run_id: "recording-one".into(),
            runtime: "deno".into(),
            env: BTreeMap::new(),
        };
        let plan: CapturePlan = serde_json::from_value(json!({
            "command": ["deno", "--cpu-prof-dir=/capture/recording-one", "run", "main.ts"],
            "env": {"HUNIN_RUN_ID": "recording-one", "HUNIN_DENO_WORKER_SESSION": "session-one"},
            "runtime_version": "deno 2.9.8",
            "session_identity": {"worker-session": "session-one"},
        }))
        .unwrap();
        let settings = plan.settings(&request);
        assert_eq!(settings["command"][1], "--cpu-prof-dir=<artifacts>");
        assert_eq!(settings["environment"]["HUNIN_RUN_ID"], "<run>");
        assert_eq!(
            settings["environment"]["HUNIN_DENO_WORKER_SESSION"],
            "<worker-session>"
        );
        assert_eq!(plan.env["HUNIN_DENO_WORKER_SESSION"], "session-one");
    }

    #[test]
    fn equivalent_bun_plans_have_equal_settings_but_distinct_launch_sessions() {
        let Some(executable) = std::env::var_os("HUNIN_TEST_BUN") else {
            eprintln!("Bun planning control requires HUNIN_TEST_BUN; not exercised on this host");
            return;
        };
        let directory = tempfile::tempdir().unwrap();
        let request = |id: &str, env| CaptureRequest {
            command: vec![
                executable.to_string_lossy().into_owned(),
                "-e".into(),
                "console.log(1)".into(),
            ],
            cwd: directory.path().to_path_buf(),
            artifact_dir: directory.path().join(id),
            run_id: id.into(),
            runtime: "bun".into(),
            env,
        };
        let first = request("recording-one", BTreeMap::new());
        let second = request("recording-two", BTreeMap::new());
        let first_plan = super::super::prepare(&first).unwrap();
        let second_plan = super::super::prepare(&second).unwrap();
        assert_ne!(
            first_plan.session_identity["bun-preload"],
            second_plan.session_identity["bun-preload"]
        );
        assert_ne!(
            first_plan.env["BUN_OPTIONS"],
            second_plan.env["BUN_OPTIONS"]
        );
        assert_eq!(first_plan.settings(&first), second_plan.settings(&second));

        let changed = request(
            "recording-three",
            BTreeMap::from([("BUN_OPTIONS".into(), "--smol".into())]),
        );
        let changed_plan = super::super::prepare(&changed).unwrap();
        assert_ne!(first_plan.settings(&first), changed_plan.settings(&changed));
        assert!(changed_plan.env["BUN_OPTIONS"].starts_with("--smol "));
    }
}

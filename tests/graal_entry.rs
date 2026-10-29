#[cfg(unix)]
mod unix {
    use hunin::{execution::Engine, storage::Store};
    use serde_json::json;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[tokio::test]
    #[ignore = "requires the selected GraalPy executable in HUNIN_TEST_GRAAL"]
    async fn graal_spawn_identity_failure_preserves_target_outcome_and_raw_profile() {
        let executable = std::env::var("HUNIN_TEST_GRAAL").expect("set HUNIN_TEST_GRAAL");
        for (blocked, exact_stdout) in [(false, false), (true, false), (false, true)] {
            let project = tempfile::tempdir().unwrap();
            let data = project.path().join("data");
            let store = Store::open(&data).unwrap();
            let engine = Engine::new(store.clone());
            // The prerequisite version probe runs after the capture workspace
            // exists. Blocking only its future sidecar gives a deterministic
            // host-evidence failure before the real runtime is spawned.
            let launcher = project.path().join("graal-launcher");
            fs::write(&launcher, format!(
                "#!/usr/bin/env python3\nimport os, pathlib, sys\nexe = {}\nif sys.argv[1:] == ['--version']:\n    for capture in pathlib.Path(__file__).parent.joinpath('data/captures').iterdir():\n        capture.joinpath('cpu.graal.meta.json').mkdir()\nos.execv(exe, [exe, *sys.argv[1:]])\n",
                serde_json::to_string(&executable).unwrap()
            )).unwrap();
            fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
            let command = if blocked {
                launcher.to_string_lossy().into_owned()
            } else {
                executable.clone()
            };
            let run = engine.start(serde_json::from_value(json!({
                "command":[command,"-c","import os, sys, time\ndeadline=time.monotonic()+0.2\nvalue=0\nwhile time.monotonic()<deadline: value+=1\nsys.stdout.buffer.write(b'target-ok\\n')\n"],
                "runtime":"graalpy", "cwd":project.path(), "expected_stdout":exact_stdout.then_some("target-ok\n"), "timeout_seconds":60,
            })).unwrap()).await.unwrap();
            let run = engine.wait(&run.id).await.unwrap();
            assert_eq!(
                run.correctness["passed"], !exact_stdout,
                "blocked={blocked}, exact_stdout={exact_stdout}: {:?}",
                run.diagnostics
            );
            assert_eq!(run.exit_code, Some(0));
            assert_eq!(
                run.source["target_outcome"]["status"],
                if exact_stdout { "failed" } else { "completed" }
            );
            let artifacts = store.artifacts(&run.id).unwrap();
            let stdout = artifacts
                .iter()
                .find(|artifact| artifact.format == "stdout")
                .unwrap();
            let stdout =
                fs::read_to_string(store.artifact_path(&run.id, &stdout.id).unwrap()).unwrap();
            // CPUSampler writes its own line to the supervised stdout. Retain
            // it exactly; never strip bytes to fabricate target correctness.
            assert!(
                stdout.starts_with("target-ok\nPrinting output to "),
                "{stdout:?}"
            );
            assert!(stdout.ends_with("/cpu.graal.json\n"), "{stdout:?}");
            assert!(
                artifacts
                    .iter()
                    .any(|artifact| artifact.format == "graal-cpusampler"),
                "raw profile was not retained: {:?}",
                run.diagnostics
            );
            if blocked {
                assert_eq!(run.status, "failed");
                assert_eq!(run.source["collection_start"]["status"], "failed");
                assert_eq!(run.source["collection_completion"]["state"], "incomplete");
                assert_eq!(run.source["collection_status"], "failed");
                assert!(
                    !store
                        .records(&run.id)
                        .unwrap()
                        .iter()
                        .any(|record| record["kind"] == "sample")
                );
            } else {
                assert_eq!(
                    run.status,
                    if exact_stdout { "failed" } else { "completed" },
                    "{:?}",
                    run.diagnostics
                );
                assert_eq!(run.source["collection_status"], "completed");
                assert_eq!(run.source["collection_completion"]["state"], "complete");
                let metadata = artifacts
                    .iter()
                    .find(|artifact| artifact.name == "cpu.graal.meta.json")
                    .unwrap();
                let metadata: serde_json::Value = serde_json::from_slice(
                    &fs::read(store.artifact_path(&run.id, &metadata.id).unwrap()).unwrap(),
                )
                .unwrap();
                assert_eq!(metadata["pid"], run.source["supervised_pid"]);
                assert_eq!(metadata["pid_source"], "supervised_runtime_process_spawn");
                assert!(metadata.get("sys.version").is_none());
                assert!(metadata.get("epoch_start_us").is_none());
                assert!(
                    store
                        .records(&run.id)
                        .unwrap()
                        .iter()
                        .any(|record| record["kind"] == "sample")
                );
            }
        }
    }
}

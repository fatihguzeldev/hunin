use hunin::{analysis::Engine, model::runs::Run, storage::Store};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, time::Duration};

fn python() -> String {
    std::env::var("HUNIN_TEST_PYTHON").unwrap_or_else(|_| {
        if cfg!(windows) {
            "python".into()
        } else {
            "python3".into()
        }
    })
}
struct Fixture {
    _root: tempfile::TempDir,
    store: Store,
    engine: Engine,
    package: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("store")).unwrap();
        let package = root.path().join("package");
        fs::create_dir(&package).unwrap();
        store
            .save_run(&Run {
                id: "recording".into(),
                name: "fixture".into(),
                status: "completed".into(),
                command: vec!["fixture".into()],
                cwd: root.path().display().to_string(),
                runtime: "fixture".into(),
                started_at_us: 10,
                finished_at_us: Some(20),
                exit_code: Some(0),
                diagnostics: vec![],
                source: json!({"fingerprint":"build-v1"}),
                correctness: json!({"passed":true}),
            })
            .unwrap();
        let records: Vec<_> = ["request","worker"].iter().enumerate().map(|(index,name)|json!({"schema_version":1,"kind":"event","id":format!("input-{index}"),"run_id":"recording","producer_id":"target-process","pid":123,"tid":"thread","timestamp_us":10+index,"name":name,"basis":"observed"})).collect();
        store
            .accept("recording", "fixture-inputs", &records)
            .unwrap();
        let engine = Engine::new(store.clone());
        Self {
            _root: root,
            store,
            engine,
            package,
        }
    }
    fn register(&self, source: &str, dependencies: Value) -> Value {
        fs::write(self.package.join("analyze.py"), source).unwrap();
        self.engine.register(&json!({"name":"relationship fixture","version":"1","command":[python(),"analyze.py"],"cwd":self.package,"requirements":{"runtime":"fixture","source_fingerprint":"build-v1"},"dependencies":dependencies})).unwrap()["analysis"].clone()
    }
    async fn start(&self, analysis: &Value, timeout: f64) -> Value {
        self.engine.run(&json!({"run_id":"recording","analysis_id":analysis["id"],"timeout_seconds":timeout})).await.unwrap()
    }
    async fn wait(&self, started: &Value) -> Value {
        tokio::time::timeout(
            Duration::from_secs(15),
            self.engine.wait(started["result_set_id"].as_str().unwrap()),
        )
        .await
        .unwrap()
        .unwrap()
    }
}
const RELATION: &str = r#"import json,sys,uuid
value=json.load(sys.stdin)
a,b=value['records'][:2]
print(json.dumps({'schema_version':1,'kind':'relation','id':str(uuid.uuid4()),'run_id':value['run']['id'],'producer_id':'analysis-fixture','pid':0,'tid':'','timestamp_us':a['timestamp_us'],'basis':'derived','from':a['id'],'to':b['id'],'relation':'dispatches','evidence':[a['id'],b['id']]}),flush=True)
"#;

#[tokio::test]
async fn managed_package_recomputes_relations_and_replays_without_double_counting() {
    let fixture = Fixture::new();
    let analysis = fixture.register(RELATION, json!({}));
    fs::remove_file(fixture.package.join("analyze.py")).unwrap();
    let started = fixture.start(&analysis, 5.0).await;
    assert_eq!(started["status"], "running");
    let accepted = fixture.wait(&started).await;
    assert_eq!(accepted["status"], "accepted", "{accepted}");
    let rows = fixture.store.records("recording").unwrap();
    assert_eq!(rows.len(), 3);
    let relation = rows.iter().find(|r| r["kind"] == "relation").unwrap();
    assert_eq!(relation["basis"], "derived");
    assert_eq!(relation["evidence"], json!(["input-0", "input-1"]));
    assert!(
        accepted["raw_artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["format"] == "analysis-input")
    );
    let replay = fixture.start(&analysis, 5.0).await;
    let replay = fixture.wait(&replay).await;
    assert_eq!(replay["status"], "replayed", "{replay}");
    assert_eq!(replay["result_set_id"], accepted["result_set_id"]);
    assert_eq!(fixture.store.records("recording").unwrap().len(), 3);
}
#[tokio::test]
async fn stale_build_and_changed_managed_code_are_rejected_before_reuse() {
    let fixture = Fixture::new();
    let analysis = fixture.register(RELATION, json!({}));
    let mut run = fixture.store.run("recording").unwrap();
    run.source["fingerprint"] = json!("build-v2");
    fixture.store.save_run(&run).unwrap();
    let error = fixture
        .engine
        .run(&json!({"run_id":"recording","analysis_id":analysis["id"]}))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("source/build applicability mismatch")
    );
    run.source["fingerprint"] = json!("build-v1");
    fixture.store.save_run(&run).unwrap();
    fs::write(
        PathBuf::from(analysis["package_dir"].as_str().unwrap()).join("source/analyze.py"),
        "changed",
    )
    .unwrap();
    let error = fixture
        .engine
        .run(&json!({"run_id":"recording","analysis_id":analysis["id"]}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("package changed"));
}
#[tokio::test]
async fn partial_output_from_failed_analyzer_is_retained_without_publication() {
    let fixture = Fixture::new();
    let analysis = fixture.register(&format!("{RELATION}\nsys.exit(7)\n"), json!({}));
    let started = fixture.start(&analysis, 5.0).await;
    let failed = fixture.wait(&started).await;
    assert_eq!(failed["status"], "failed", "{failed}");
    assert_eq!(fixture.store.records("recording").unwrap().len(), 2);
    assert!(
        failed["raw_artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == "stdout.jsonl" && a["size"].as_u64().unwrap() > 0)
    );
}
#[tokio::test]
async fn derived_rows_without_input_evidence_cannot_be_published() {
    let fixture = Fixture::new();
    let source = RELATION.replace(
        "'evidence':[a['id'],b['id']]",
        "'evidence':['invented-input']",
    );
    let analysis = fixture.register(&source, json!({}));
    let started = fixture.start(&analysis, 5.0).await;
    let rejected = fixture.wait(&started).await;
    assert_eq!(rejected["status"], "failed", "{rejected}");
    assert!(
        rejected["diagnostics"]
            .to_string()
            .contains("input observation")
    );
    assert_eq!(fixture.store.records("recording").unwrap().len(), 2);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_and_timeout_stop_owned_analyzers_without_accepting_partial_rows() {
    let fixture = Fixture::new();
    let analysis = fixture.register(
        &format!(
            "{RELATION}\nimport time\nprint('ready',file=sys.stderr,flush=True)\ntime.sleep(60)\n"
        ),
        json!({}),
    );
    let started = fixture.start(&analysis, 10.0).await;
    let id = started["result_set_id"].as_str().unwrap();
    let directory = PathBuf::from(
        fixture.store.document("analysis-results", id).unwrap()["execution_dir"]
            .as_str()
            .unwrap(),
    );
    for _ in 0..200 {
        if fs::read_to_string(directory.join("stderr.txt"))
            .unwrap_or_default()
            .contains("ready")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    fixture.engine.cancel(id).await.unwrap();
    let cancelled = fixture.wait(&started).await;
    assert_eq!(cancelled["status"], "cancelled", "{cancelled}");
    let timed = fixture.start(&analysis, 0.1).await;
    let timed = fixture.wait(&timed).await;
    assert_eq!(timed["status"], "timed_out", "{timed}");
    assert_eq!(fixture.store.records("recording").unwrap().len(), 2);
}
#[tokio::test]
async fn output_budget_rejects_overflow_and_preserves_bounded_raw_evidence() {
    let fixture = Fixture::new();
    let analysis = fixture.register(
        &format!("{RELATION}\nsys.stderr.write('x'*(20*1024*1024))\nsys.stderr.flush()\n"),
        json!({}),
    );
    let started = fixture.start(&analysis, 5.0).await;
    let failed = fixture.wait(&started).await;
    assert_eq!(failed["status"], "failed", "{failed}");
    assert!(failed["diagnostics"].to_string().contains("16 MiB"));
    assert_eq!(fixture.store.records("recording").unwrap().len(), 2);
    let total: u64 = failed["raw_artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["name"] == "stdout.jsonl" || a["name"] == "stderr.txt")
        .map(|a| a["size"].as_u64().unwrap())
        .sum();
    assert!(total <= 16 * 1024 * 1024);
}
#[tokio::test]
async fn declared_setup_runs_in_managed_workspace_and_retains_dependency_identity() {
    let fixture = Fixture::new();
    fs::write(
        fixture.package.join("setup.py"),
        "from pathlib import Path\nPath('support.py').write_text('VALUE=23')\n",
    )
    .unwrap();
    let analysis = fixture.register(
        &format!("import support\nassert support.VALUE==23\n{RELATION}"),
        json!({"setup_commands":[[python(),"setup.py"]]}),
    );
    let started = fixture.start(&analysis, 5.0).await;
    let accepted = fixture.wait(&started).await;
    assert_eq!(accepted["status"], "accepted", "{accepted}");
    assert!(!fixture.package.join("support.py").exists());
    assert!(
        !PathBuf::from(analysis["package_dir"].as_str().unwrap())
            .join("source/support.py")
            .exists()
    );
    assert!(
        accepted["raw_artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == "dependencies.json")
    );
}
#[tokio::test]
async fn locked_python_dependencies_install_into_an_isolated_managed_environment() {
    let fixture = Fixture::new();
    fs::write(
        fixture.package.join("requirements.lock"),
        "# no third-party packages required\n",
    )
    .unwrap();
    let analysis = fixture.register(
        &format!("import sys\nassert sys.prefix!=sys.base_prefix\n{RELATION}"),
        json!({"python_requirements":"requirements.lock"}),
    );
    let started = fixture.start(&analysis, 10.0).await;
    let accepted = fixture.wait(&started).await;
    assert_eq!(accepted["status"], "accepted", "{accepted}");
    assert_eq!(fixture.store.records("recording").unwrap().len(), 3);
}

#[test]
fn unknown_applicability_and_unmanaged_package_paths_are_rejected() {
    let fixture = Fixture::new();
    fs::write(fixture.package.join("analyze.py"), RELATION).unwrap();
    let base = json!({"name":"scope boundary","command":[python(),"analyze.py"],"cwd":fixture.package,"requirements":{}});
    assert!(
        fixture
            .engine
            .register(&base)
            .unwrap_err()
            .to_string()
            .contains("applicability runtime")
    );
    let mut unknown = base.clone();
    unknown["requirements"] = json!({"runtime":"fixture","source_fingerprint":"unknown"});
    assert!(fixture.engine.register(&unknown).is_err());
    let mut traversal = base.clone();
    traversal["requirements"] = json!({"runtime":"*","source_independent":true});
    traversal["files"] = json!(["../outside.py"]);
    assert!(fixture.engine.register(&traversal).is_err());
    #[cfg(unix)]
    {
        let outside = fixture._root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("analyze.py"), RELATION).unwrap();
        std::os::unix::fs::symlink(outside, fixture.package.join("linked")).unwrap();
        let mut escaped = base;
        escaped["requirements"] = json!({"runtime":"*","source_independent":true});
        escaped["files"] = json!(["linked/analyze.py"]);
        assert!(
            fixture
                .engine
                .register(&escaped)
                .unwrap_err()
                .to_string()
                .contains("symlink")
        );
    }
}

#[tokio::test]
async fn locked_node_dependencies_and_non_target_language_analyzer_execute_from_managed_package() {
    let fixture = Fixture::new();
    fs::write(
        fixture.package.join("package.json"),
        r#"{"name":"hunin-analyzer-fixture","version":"1.0.0","private":true}"#,
    )
    .unwrap();
    fs::write(fixture.package.join("package-lock.json"),r#"{"name":"hunin-analyzer-fixture","version":"1.0.0","lockfileVersion":3,"requires":true,"packages":{"":{"name":"hunin-analyzer-fixture","version":"1.0.0"}}}"#).unwrap();
    fs::write(fixture.package.join("analyze.mjs"),r#"import {readFileSync} from 'node:fs';import{randomUUID}from'node:crypto';const v=JSON.parse(readFileSync(0,'utf8'));const[a,b]=v.records;console.log(JSON.stringify({schema_version:1,kind:'relation',id:randomUUID(),run_id:v.run.id,producer_id:'node-analyzer',pid:0,tid:'',timestamp_us:a.timestamp_us,basis:'derived',from:a.id,to:b.id,relation:'links',evidence:[a.id,b.id]}));"#).unwrap();
    let node = std::env::var("HUNIN_TEST_NODE").unwrap_or_else(|_| "node".into());
    let analysis=fixture.engine.register(&json!({"name":"Node custom analysis","command":[node,"analyze.mjs"],"cwd":fixture.package,"requirements":{"runtime":"fixture","source_fingerprint":"build-v1"},"dependencies":{"node_lockfile":"package-lock.json"}})).unwrap()["analysis"].clone();
    let started = fixture.start(&analysis, 10.0).await;
    let accepted = fixture.wait(&started).await;
    assert_eq!(accepted["status"], "accepted", "{accepted}");
    assert_eq!(fixture.store.records("recording").unwrap().len(), 3);
    assert!(!fixture.package.join("node_modules").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistent_cancellation_and_instance_shutdown_preserve_foreign_ownership() {
    let fixture = Fixture::new();
    let analysis = fixture.register(
        &format!("{RELATION}\nimport time\ntime.sleep(60)\n"),
        json!({}),
    );
    let other = Engine::new(fixture.store.clone());
    let own = fixture.start(&analysis, 10.0).await;
    let foreign = other
        .run(&json!({"run_id":"recording","analysis_id":analysis["id"],"timeout_seconds":10}))
        .await
        .unwrap();
    fixture.engine.shutdown().await.unwrap();
    assert_eq!(fixture.wait(&own).await["status"], "cancelled");
    assert_eq!(
        fixture
            .store
            .document(
                "analysis-results",
                foreign["result_set_id"].as_str().unwrap()
            )
            .unwrap()["status"],
        "running"
    );
    assert!(
        fixture
            .engine
            .run(&json!({"run_id":"recording","analysis_id":analysis["id"]}))
            .await
            .is_err()
    );
    fixture
        .engine
        .cancel(foreign["result_set_id"].as_str().unwrap())
        .await
        .unwrap();
    let stopped = tokio::time::timeout(
        Duration::from_secs(5),
        other.wait(foreign["result_set_id"].as_str().unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(stopped["status"], "cancelled", "{stopped}");
    other.shutdown().await.unwrap();
    assert_eq!(fixture.store.records("recording").unwrap().len(), 2);
}

#[test]
fn confirmed_dead_owner_recovery_retains_partial_evidence_and_unblocks_removal() {
    let fixture = Fixture::new();
    let mut departed = std::process::Command::new(python())
        .args(["-c", "pass"])
        .spawn()
        .unwrap();
    let pid = departed.id();
    departed.wait().unwrap();
    let directory = fixture._root.path().join("abandoned");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("input.json"), "{}").unwrap();
    fs::write(directory.join("stdout.jsonl"), "partial").unwrap();
    fixture.store.save_document("analysis-results","abandoned",&json!({"id":"abandoned","result_set_id":"abandoned","run_id":"recording","status":"running","owner":{"pid":pid,"process_start_time_seconds":1,"instance_id":"departed-owner"},"execution_dir":directory})).unwrap();
    assert_eq!(fixture.engine.recover().unwrap(), 1);
    let recovered = fixture
        .store
        .document("analysis-results", "abandoned")
        .unwrap();
    assert_eq!(recovered["status"], "interrupted");
    assert!(recovered["raw_artifacts"].as_array().unwrap().len() >= 2);
    assert_eq!(fixture.store.records("recording").unwrap().len(), 2);
    fixture.store.remove_run("recording").unwrap();
}

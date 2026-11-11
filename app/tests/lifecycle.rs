use hunin_app::{BeginCapture, CapturePlan};
use hunin_core::{
    control::{Controller, RequestId},
    project::Project,
    query::{Query, QueryLimits},
    records::SourceDefinition,
};
use hunin_runtimes::RuntimeKind;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

fn plan(project: &Project, workload: &Path) -> CapturePlan {
    let investigation = project
        .create_investigation("i", "lifecycle", json!({}))
        .unwrap();
    let run = project.create_run(&investigation, "r", json!({})).unwrap();
    CapturePlan {
        request: RequestId {
            project: project.id().into(),
            caller: "agent".into(),
            id: "capture".into(),
        },
        run,
        runtime: RuntimeKind::CPython,
        executable: PathBuf::from(
            std::env::var_os("HUNIN_PYTHON").expect("set selected HUNIN_PYTHON"),
        ),
        workload: workload.into(),
        conditions: json!({}),
        environment: std::collections::BTreeMap::new(),
        read_paths: vec![],
        write_paths: vec![],
        xdebug_extension: None,
    }
}

#[test]
#[ignore = "requires selected HUNIN_PYTHON"]
fn failed_half_preparation_closes_its_sources_and_leaves_preexisting_source_alone() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "p").unwrap();
    let workload = directory.path().join("work.py");
    fs::write(&workload, "def run(): return 1\n").unwrap();
    let plan = plan(&project, &workload);
    let request = plan.request.clone();
    let run = plan.run.clone();
    let prefix = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!({"caller":request.caller,"id":request.id})).unwrap()
        )
    );
    let existing = project
        .register_source(
            &run,
            SourceDefinition {
                id: format!("code-{prefix}"),
                target: None,
                schemas: vec![],
                clocks: vec![],
                interpretation: "unrelated-existing-source".into(),
                metadata: json!({}),
            },
        )
        .unwrap();
    assert!(hunin_app::begin(project.clone(), plan).is_err());
    let sources = project.sources(&run, QueryLimits::default(), None).unwrap();
    assert_eq!(sources.items.len(), 2);
    assert!(
        sources
            .items
            .iter()
            .find(|source| source.id.starts_with("resources-"))
            .unwrap()
            .closed
    );
    assert!(!existing.status().unwrap().closed);
    let outcome = Controller::new(project.clone()).lookup(&request).unwrap();
    assert_eq!(outcome.state, "failed");
    assert_eq!(outcome.result.unwrap()["cleanup_pending"], false);
    assert_eq!(
        fs::read_dir(project.directory().join("tmp"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
#[ignore = "requires selected HUNIN_PYTHON"]
fn application_failure_cannot_become_a_successful_capture_outcome() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "p").unwrap();
    let workload = directory.path().join("work.py");
    fs::write(
        &workload,
        "def run(): raise RuntimeError('controlled failure')\n",
    )
    .unwrap();
    let plan = plan(&project, &workload);
    let run = plan.run.clone();
    let BeginCapture::Started(mut capture) = hunin_app::begin(project.clone(), plan).unwrap()
    else {
        panic!("fresh capture")
    };
    capture.release().unwrap();
    let report = capture.finish(Duration::from_secs(10)).unwrap();
    assert_eq!(report.application["outcome"], "application_failed");
    assert_eq!(report.request.state, "failed");
    assert!(
        !project
            .query(&Query::new(run), QueryLimits::default(), None)
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
#[ignore = "requires selected HUNIN_PYTHON"]
fn resolved_request_retry_survives_deleted_original_workload_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "p").unwrap();
    let workload = directory.path().join("work.py");
    fs::write(&workload, "def run(): return 1\n").unwrap();
    let request = plan(&project, &workload).request;
    let BeginCapture::Started(mut capture) =
        hunin_app::begin(project.clone(), plan(&project, &workload)).unwrap()
    else {
        panic!("fresh capture")
    };
    capture.release().unwrap();
    capture.finish(Duration::from_secs(10)).unwrap();
    fs::remove_file(&workload).unwrap();
    drop(project);
    let reopened = Project::open(directory.path(), "p").unwrap();
    let BeginCapture::Existing(existing) =
        hunin_app::begin(reopened.clone(), plan(&reopened, &workload)).unwrap()
    else {
        panic!("resolved identity must not launch again")
    };
    assert_eq!(existing.request, request);
    assert_eq!(existing.state, "succeeded");
}

#[test]
#[ignore = "requires selected HUNIN_PYTHON"]
fn terminal_control_fences_work_release_and_drop_confirms_owned_cleanup() {
    use hunin_core::control::Action;
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "p").unwrap();
    let workload = directory.path().join("work.py");
    let marker = directory.path().join("ran");
    fs::write(
        &workload,
        format!(
            "def run():\n    open({:?}, 'w').write('ran')\n    return 1\n",
            marker.to_str().unwrap()
        ),
    )
    .unwrap();
    let plan = plan(&project, &workload);
    let request = plan.request.clone();
    let run = plan.run.clone();
    let BeginCapture::Started(mut capture) = hunin_app::begin(project.clone(), plan).unwrap()
    else {
        panic!("fresh capture")
    };
    let pid = capture.resource_source().definition().unwrap().metadata["instance"]["pid"]
        .as_u64()
        .unwrap() as u32;
    let controller = Controller::new(project.clone());
    controller
        .accept(
            &RequestId {
                project: "p".into(),
                caller: "other-controller".into(),
                id: "stop".into(),
            },
            &run,
            Action::Cancel,
            json!({}),
        )
        .unwrap();
    assert!(capture.release().is_err());
    drop(capture);
    assert!(!marker.exists());
    assert!(hunin_platforms::inspect_process(pid).is_err());
    assert_eq!(controller.lookup(&request).unwrap().state, "reconciled");
}

#[test]
#[ignore = "requires selected HUNIN_PYTHON"]
fn cancellation_before_and_during_work_has_a_durable_cleanup_outcome() {
    for release in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let project = Project::create(directory.path(), "p").unwrap();
        let workload = directory.path().join("work.py");
        fs::write(&workload, "def run():\n    while True: pass\n").unwrap();
        let plan = plan(&project, &workload);
        let request = plan.request.clone();
        let run = plan.run.clone();
        let BeginCapture::Started(mut capture) = hunin_app::begin(project.clone(), plan).unwrap()
        else {
            panic!("fresh capture")
        };
        let pid = capture.resource_source().definition().unwrap().metadata["instance"]["pid"]
            .as_u64()
            .unwrap() as u32;
        let foreign = RequestId {
            project: "foreign".into(),
            caller: "agent".into(),
            id: "cancel".into(),
        };
        assert!(capture.cancel(foreign).is_err());
        assert!(hunin_platforms::inspect_process(pid).is_ok());
        if release {
            capture.release().unwrap();
        }
        let cancel = RequestId {
            project: "p".into(),
            caller: "agent".into(),
            id: "cancel".into(),
        };
        let outcome = capture.cancel(cancel.clone()).unwrap();
        assert_eq!(outcome.state, "succeeded");
        assert_eq!(outcome.result.as_ref().unwrap()["cleanup_pending"], false);
        assert!(hunin_platforms::inspect_process(pid).is_err());
        assert!(
            project
                .sources(&run, QueryLimits::default(), None)
                .unwrap()
                .items
                .iter()
                .all(|source| source.closed)
        );
        drop(capture);
        drop(project);
        let reopened = Project::open(directory.path(), "p").unwrap();
        let controller = Controller::new(reopened);
        assert_eq!(controller.lookup(&cancel).unwrap().state, "succeeded");
        assert_eq!(controller.lookup(&request).unwrap().state, "reconciled");
    }
}

#[test]
#[ignore = "requires selected HUNIN_PYTHON"]
fn a_failed_recording_keeps_valid_resources_and_closes_the_failed_source() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "p").unwrap();
    let workload = directory.path().join("work.py");
    fs::write(&workload, "def run(): return 1\n").unwrap();
    let plan = plan(&project, &workload);
    let request = plan.request.clone();
    let run = plan.run.clone();
    let BeginCapture::Started(mut capture) = hunin_app::begin(project.clone(), plan).unwrap()
    else {
        panic!("fresh capture")
    };
    capture.release().unwrap();
    let capture_area = fs::read_dir(project.directory().join("tmp"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let report = capture_area.join("runtime-report.json");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !report.exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    fs::write(capture_area.join("runtime-profile.json"), "{truncated").unwrap();
    assert!(capture.finish(Duration::from_secs(10)).is_err());
    assert_eq!(
        Controller::new(project.clone())
            .lookup(&request)
            .unwrap()
            .state,
        "failed"
    );
    assert!(
        project
            .sources(&run, QueryLimits::default(), None)
            .unwrap()
            .items
            .iter()
            .all(|source| source.closed)
    );
    let mut query = Query::new(run);
    query.schema = Some("platform.process_resources".into());
    assert!(
        !project
            .query(&query, QueryLimits::default(), None)
            .unwrap()
            .records
            .is_empty()
    );
    let outcome = Controller::new(project.clone()).lookup(&request).unwrap();
    let retained = PathBuf::from(
        outcome.result.as_ref().unwrap()["temporary_area"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(outcome.result.as_ref().unwrap()["cleanup_pending"], false);
    assert_eq!(
        outcome.result.as_ref().unwrap()["source_inputs_retained_for_retry"],
        true
    );
    assert!(retained.join("runtime-profile.json").is_file());
    assert!(retained.starts_with(project.directory().join("tmp")));
}

#[test]
#[cfg(unix)]
#[ignore = "requires selected HUNIN_PYTHON and POSIX fork/setsid"]
fn uncertain_cancel_retries_cleanup_without_terminating_a_detached_pipe_holder() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "p").unwrap();
    let workload = directory.path().join("work.py");
    fs::write(&workload,"import os, time\ndef run():\n    child = os.fork()\n    if child == 0:\n        os.setsid()\n        time.sleep(1.5)\n        os._exit(0)\n    return {'detached_pid': child}\n").unwrap();
    let plan = plan(&project, &workload);
    let request = plan.request.clone();
    let BeginCapture::Started(mut capture) = hunin_app::begin(project.clone(), plan).unwrap()
    else {
        panic!("fresh capture")
    };
    capture.release().unwrap();
    let capture_area = fs::read_dir(project.directory().join("tmp"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let report_path = capture_area.join("runtime-report.json");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !report_path.exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(report_path).unwrap()).unwrap();
    let detached = report["output"]["detached_pid"].as_u64().unwrap() as u32;
    let cancel = RequestId {
        project: "p".into(),
        caller: "agent".into(),
        id: "cancel".into(),
    };
    let first = capture.cancel(cancel.clone()).unwrap();
    assert_eq!(first.state, "uncertain");
    assert!(
        hunin_platforms::inspect_process(detached).is_ok(),
        "a detached descriptor holder is outside the accepted process group"
    );
    assert!(capture_area.exists());
    assert_eq!(
        Controller::new(project.clone())
            .lookup(&request)
            .unwrap()
            .state,
        "uncertain"
    );
    std::thread::sleep(Duration::from_millis(1600));
    let resolved = capture.cancel(cancel).unwrap();
    assert_eq!(resolved.state, "reconciled");
    assert_eq!(resolved.result.unwrap()["cleanup_pending"], false);
    assert_eq!(
        Controller::new(project).lookup(&request).unwrap().state,
        "reconciled"
    );
}

#[test]
#[cfg(unix)]
#[ignore = "requires selected HUNIN_PYTHON and POSIX fork/setsid"]
fn imported_capture_keeps_scratch_while_a_detached_descriptor_holder_is_unconfirmed() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "p").unwrap();
    let workload = directory.path().join("work.py");
    fs::write(&workload,"import os, time\ndef run():\n    child = os.fork()\n    if child == 0:\n        os.setsid()\n        time.sleep(2)\n        with open('descriptor-holder-done', 'w') as marker: marker.write('done')\n        os._exit(0)\n    return {'detached_pid': child}\n").unwrap();
    let plan = plan(&project, &workload);
    let request = plan.request.clone();
    let run = plan.run.clone();
    let BeginCapture::Started(mut capture) = hunin_app::begin(project.clone(), plan).unwrap()
    else {
        panic!("fresh capture")
    };
    let area = fs::read_dir(project.directory().join("tmp"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    capture.release().unwrap();
    let report = capture.finish(Duration::from_secs(10)).unwrap();
    assert!(report.application_success);
    assert!(!report.collection_complete);
    assert_eq!(report.request.state, "uncertain");
    assert!(
        area.exists(),
        "unconfirmed descriptor owner can still use scratch"
    );
    let retained = report.request.result.as_ref().unwrap()["late_completion"]["temporary_area"]
        .as_str()
        .expect("uncertain finalization must durably locate retained scratch");
    assert_eq!(Path::new(retained), area);
    let reopened = Project::open(directory.path(), "p").unwrap();
    let durable = Controller::new(reopened).lookup(&request).unwrap();
    assert_eq!(durable.state, "uncertain");
    assert_eq!(durable.result, report.request.result);
    let detached = report.application["output"]["detached_pid"]
        .as_u64()
        .unwrap() as u32;
    assert!(hunin_platforms::inspect_process(detached).is_ok());
    assert!(
        project
            .sources(&run, QueryLimits::default(), None)
            .unwrap()
            .items
            .iter()
            .all(|source| source.closed)
    );
    assert!(
        !project
            .query(&Query::new(run), QueryLimits::default(), None)
            .unwrap()
            .records
            .is_empty()
    );
    let marker = area.join("descriptor-holder-done");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    while hunin_platforms::inspect_process(detached).is_ok() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fs::read_to_string(&marker).unwrap(), "done");
    // The last-use marker and process disappearance are caller-owned reconciliation evidence.
    fs::remove_dir_all(&area).unwrap();
    let controller = Controller::new(project);
    assert_eq!(controller.reconcile(&request,json!({"application_owned_scratch_use_finished":true,"temporary_area_removed":area}),true).unwrap().state,"reconciled");
}

#[test]
#[cfg(unix)]
fn readiness_failure_records_failed_intent_and_removes_preparation_scratch() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "p").unwrap();
    let investigation = project
        .create_investigation("i", "failed readiness", json!({}))
        .unwrap();
    let run = project.create_run(&investigation, "r", json!({})).unwrap();
    let workload = directory.path().join("work.py");
    fs::write(&workload, "def run(): return 1\n").unwrap();
    let request = RequestId {
        project: "p".into(),
        caller: "agent".into(),
        id: "readiness".into(),
    };
    let plan = || CapturePlan {
        request: request.clone(),
        run: run.clone(),
        runtime: RuntimeKind::CPython,
        executable: PathBuf::from("/usr/bin/false"),
        workload: workload.clone(),
        conditions: json!({}),
        environment: std::collections::BTreeMap::new(),
        read_paths: vec![],
        write_paths: vec![],
        xdebug_extension: None,
    };
    assert!(hunin_app::begin(project.clone(), plan()).is_err());
    let outcome = Controller::new(project.clone()).lookup(&request).unwrap();
    assert_eq!(outcome.state, "failed");
    assert_eq!(outcome.result.unwrap()["cleanup_pending"], false);
    assert!(
        project
            .sources(&run, QueryLimits::default(), None)
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        fs::read_dir(project.directory().join("tmp"))
            .unwrap()
            .count(),
        0
    );
    assert!(matches!(
        hunin_app::begin(project, plan()).unwrap(),
        BeginCapture::Existing(_)
    ));
}

#[cfg(any(unix, windows))]
struct DeleteBlocker {
    #[cfg(unix)]
    directory: PathBuf,
    #[cfg(windows)]
    _file: fs::File,
}

#[cfg(any(unix, windows))]
impl DeleteBlocker {
    fn new(area: &Path) -> Self {
        let directory = area.join("protected");
        fs::create_dir(&directory).unwrap();
        let file = directory.join("retained");
        fs::write(&file, "owned scratch").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o500)).unwrap();
            assert!(fs::remove_file(&file).is_err());
            Self { directory }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let handle = fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&file)
                .unwrap();
            assert!(fs::remove_file(&file).is_err());
            Self { _file: handle }
        }
    }
}

#[cfg(unix)]
impl Drop for DeleteBlocker {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700));
    }
}

#[test]
#[cfg(any(unix, windows))]
#[ignore = "requires selected HUNIN_PYTHON and enforced delete permissions or share modes"]
fn failed_finalization_delete_retains_a_durable_cleanup_locator() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "p").unwrap();
    let workload = directory.path().join("work.py");
    fs::write(&workload, "def run(): return 42\n").unwrap();
    let plan = plan(&project, &workload);
    let request = plan.request.clone();
    let run = plan.run.clone();
    let BeginCapture::Started(mut capture) = hunin_app::begin(project.clone(), plan).unwrap()
    else {
        panic!("fresh capture")
    };
    let area = fs::read_dir(project.directory().join("tmp"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    capture.release().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !area.join("runtime-report.json").exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let blocker = DeleteBlocker::new(&area);
    let result = capture.finish(Duration::from_secs(5));
    drop(blocker);
    assert!(result.is_err());
    assert!(area.exists());
    let reopened = Project::open(directory.path(), "p").unwrap();
    let controller = Controller::new(reopened.clone());
    let outcome = controller.lookup(&request).unwrap();
    assert_eq!(outcome.state, "uncertain");
    assert_eq!(outcome.result.as_ref().unwrap()["cleanup_pending"], true);
    assert_eq!(
        outcome.result.as_ref().unwrap()["late_completion"]["temporary_area"],
        area.to_string_lossy().as_ref()
    );
    assert!(
        !reopened
            .query(&Query::new(run), QueryLimits::default(), None)
            .unwrap()
            .records
            .is_empty()
    );
    fs::remove_dir_all(&area).unwrap();
    assert_eq!(
        controller
            .reconcile(&request, json!({"temporary_area_removed":area}), true)
            .unwrap()
            .state,
        "reconciled"
    );
}

#[test]
#[cfg(any(unix, windows))]
#[ignore = "requires selected HUNIN_PYTHON and enforced delete permissions or share modes"]
fn failed_cancellation_delete_keeps_its_locator_across_cleanup_retries() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "p").unwrap();
    let workload = directory.path().join("work.py");
    fs::write(
        &workload,
        "import time\ndef run():\n    time.sleep(5)\n    return 42\n",
    )
    .unwrap();
    let plan = plan(&project, &workload);
    let request = plan.request.clone();
    let BeginCapture::Started(mut capture) = hunin_app::begin(project.clone(), plan).unwrap()
    else {
        panic!("fresh capture")
    };
    capture.release().unwrap();
    let area = fs::read_dir(project.directory().join("tmp"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let blocker = DeleteBlocker::new(&area);
    let cancel = RequestId {
        project: "p".into(),
        caller: "agent".into(),
        id: "cancel".into(),
    };
    let first = capture.cancel(cancel.clone());
    let second = capture.cancel(cancel.clone());
    drop(blocker);
    for outcome in [first.unwrap(), second.unwrap()] {
        assert_eq!(outcome.state, "uncertain");
        assert_eq!(outcome.result.as_ref().unwrap()["cleanup_pending"], true);
        assert_eq!(
            outcome.result.as_ref().unwrap()["late_completion"]["temporary_area"],
            area.to_string_lossy().as_ref()
        );
    }
    let reopened = Project::open(directory.path(), "p").unwrap();
    assert_eq!(
        Controller::new(reopened).lookup(&cancel).unwrap().state,
        "uncertain"
    );
    let resolved = capture.cancel(cancel).unwrap();
    assert_eq!(resolved.state, "reconciled");
    assert_eq!(resolved.result.unwrap()["cleanup_pending"], false);
    assert!(!area.exists());
    assert_eq!(
        Controller::new(project).lookup(&request).unwrap().state,
        "reconciled"
    );
}

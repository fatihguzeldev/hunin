use hunin_core::project::Project;
use serde_json::json;

#[test]
fn project_identity_is_not_repository_path_and_survives_reopen() {
    let data = tempfile::tempdir().unwrap();
    let a = Project::create(data.path(), "a").unwrap();
    let b = Project::create(data.path(), "b").unwrap();
    let repository = json!({"repository":"/same/repo"});
    a.set_metadata(repository.clone()).unwrap();
    b.set_metadata(repository).unwrap();
    let ia = a
        .create_investigation("same-id", "a's question", json!({}))
        .unwrap();
    let ib = b
        .create_investigation("same-id", "b's question", json!({}))
        .unwrap();
    assert!(b.create_run(&ia, "wrong", json!({})).is_err());
    a.create_run(&ia, "run", json!({"input":"a"})).unwrap();
    b.create_run(&ib, "run", json!({"input":"b"})).unwrap();
    a.set_metadata(json!({"repository":"/moved/repo"})).unwrap();
    drop(a);
    drop(b);
    let a = Project::open(data.path(), "a").unwrap();
    let b = Project::open(data.path(), "b").unwrap();
    assert_eq!(a.run_conditions(&a.reference("run")).unwrap()["input"], "a");
    assert_eq!(b.run_conditions(&b.reference("run")).unwrap()["input"], "b");
    assert_eq!(a.metadata().unwrap()["repository"], "/moved/repo");
    assert_eq!(a.id(), "a");
    assert!(a.directory().is_absolute());
}

#[test]
fn wrong_database_identity_and_path_escape_fail_before_mutation() {
    let data = tempfile::tempdir().unwrap();
    let a = Project::create(data.path(), "a").unwrap();
    a.set_metadata(json!({"sentinel":777})).unwrap();
    let original = a.directory().join("project.sqlite");
    drop(a);
    let b = data.path().join("projects/b");
    std::fs::create_dir_all(&b).unwrap();
    std::fs::copy(&original, b.join("project.sqlite")).unwrap();
    assert!(Project::open(data.path(), "b").is_err());
    assert!(Project::create(data.path(), "../escape").is_err());
    assert_eq!(
        Project::open(data.path(), "a").unwrap().metadata().unwrap()["sentinel"],
        777
    );
}

#[cfg(unix)]
#[test]
fn temporary_area_cannot_redirect_cleanup_outside_project() {
    let data = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let p = Project::create(data.path(), "a").unwrap();
    let tmp = p.directory().join("tmp");
    drop(p);
    std::fs::remove_dir(&tmp).unwrap();
    std::os::unix::fs::symlink(outside.path(), &tmp).unwrap();
    assert!(Project::open(data.path(), "a").is_err());
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
}

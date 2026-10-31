use super::*;
use crate::{
    model::runs::{Run, RunStatus},
    storage::Store,
};
use rusqlite::Connection;
use serde_json::json;
use std::{
    fs,
    path::Path,
    sync::{Arc, Barrier},
};

fn recording(name: &str) -> Run {
    Run {
        id: "same-recording-id".into(),
        name: name.into(),
        status: RunStatus::Completed,
        command: vec![],
        cwd: "/original/measured/checkout".into(),
        runtime: "import".into(),
        started_at_us: 1,
        finished_at_us: Some(2),
        exit_code: Some(0),
        diagnostics: vec![],
        source: json!({"revision":"original"}),
        correctness: json!({}),
    }
}

#[test]
fn retained_store_handles_cannot_write_to_a_replacement_or_recreate_a_missing_database() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ProjectRegistry::open(temp.path()).unwrap();
    let first = registry.create("first", None).unwrap();
    let second = registry.create("second", None).unwrap();
    let retained = registry.open_store(&first).unwrap();
    registry
        .open_store(&second)
        .unwrap()
        .save_run(&recording("second"))
        .unwrap();
    let moved = temp.path().join("moved-first");
    fs::rename(&first.store_path, &moved).unwrap();
    fs::rename(&second.store_path, &first.store_path).unwrap();
    assert!(
        retained
            .with_query_snapshot(
                "same-recording-id",
                crate::storage::ReadLimits::default(),
                || Ok(()),
                |_| Ok(())
            )
            .is_err()
    );
    assert!(
        retained
            .inspect(crate::storage::InspectionLimits::default())
            .is_err()
    );
    assert!(retained.save_run(&recording("wrong project")).is_err());
    assert_eq!(
        Store::open(&first.store_path)
            .unwrap()
            .run("same-recording-id")
            .unwrap()
            .name,
        "second"
    );
    fs::rename(&first.store_path, &second.store_path).unwrap();
    assert!(retained.save_run(&recording("missing project")).is_err());
    assert!(!first.store_path.exists());
    fs::rename(&moved, &first.store_path).unwrap();
    retained.save_run(&recording("restored original")).unwrap();
}

#[test]
fn retained_legacy_store_cannot_keep_writing_after_explicit_project_adoption() {
    let temp = tempfile::tempdir().unwrap();
    Store::open(temp.path()).unwrap();
    let registry = ProjectRegistry::open(temp.path()).unwrap();
    let legacy = registry
        .open_store(&registry.get("legacy").unwrap())
        .unwrap();
    let project = registry.register("adopted", temp.path(), None).unwrap();
    assert!(legacy.save_run(&recording("old legacy context")).is_err());
    let selected = registry.open_store(&project).unwrap();
    assert!(selected.runs(10, 0).unwrap().is_empty());
    selected.save_run(&recording("adopted context")).unwrap();
}

#[test]
fn projects_own_independent_recordings_and_artifacts_without_id_filters() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ProjectRegistry::open(temp.path()).unwrap();
    let first = registry.create("ring3", None).unwrap();
    let second = registry.create("hunin", None).unwrap();
    assert_eq!(first.store_path.file_name().unwrap(), "ring3");
    assert_ne!(first.id, second.id);
    for (project, name, bytes) in [
        (&first, "first", b"first".as_slice()),
        (&second, "second", b"second".as_slice()),
    ] {
        let store = registry.open_store(project).unwrap();
        store.save_run(&recording(name)).unwrap();
        let artifact = store
            .add_artifact_bytes("same-recording-id", "raw", "profile", bytes)
            .unwrap();
        assert_eq!(
            fs::read(
                store
                    .artifact_path("same-recording-id", &artifact.id)
                    .unwrap()
            )
            .unwrap(),
            bytes
        );
    }
    let store = registry.open_store(&first).unwrap();
    assert_eq!(store.run("same-recording-id").unwrap().name, "first");
    assert_eq!(
        registry
            .open_store(&second)
            .unwrap()
            .run("same-recording-id")
            .unwrap()
            .name,
        "second"
    );
    let connection = Connection::open(first.store_path.join("hunin.sqlite")).unwrap();
    let columns = connection
        .prepare("PRAGMA table_info(runs)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(!columns.iter().any(|column| column == "project_id"));
}

#[test]
fn registry_listing_does_not_open_migrate_or_recreate_project_databases() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ProjectRegistry::open(temp.path()).unwrap();
    let first = registry.create("present", None).unwrap();
    let second = registry.create("missing", None).unwrap();
    fs::write(first.store_path.join("hunin.sqlite"), b"not a database").unwrap();
    fs::remove_dir_all(&second.store_path).unwrap();
    assert_eq!(registry.list().unwrap().len(), 2);
    assert_eq!(registry.get(&first.id).unwrap(), first);
    assert!(registry.open_store(&first).is_err());
    assert!(registry.open_store(&second).is_err());
    assert!(!second.store_path.exists());
}

#[test]
fn replaced_store_is_rejected_before_recording_migration() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ProjectRegistry::open(temp.path()).unwrap();
    let first = registry.create("first", None).unwrap();
    let second = registry.create("second", None).unwrap();
    let connection = Connection::open(second.store_path.join("hunin.sqlite")).unwrap();
    connection.execute_batch("PRAGMA user_version=99;").unwrap();
    drop(connection);
    fs::rename(&first.store_path, temp.path().join("first-original")).unwrap();
    fs::rename(&second.store_path, &first.store_path).unwrap();
    assert!(
        registry
            .open_store(&first)
            .err()
            .unwrap()
            .to_string()
            .contains("identity mismatch")
    );
    let connection = Connection::open(first.store_path.join("hunin.sqlite")).unwrap();
    assert_eq!(
        connection
            .query_row::<u32, _, _>("PRAGMA user_version", [], |row| row.get(0))
            .unwrap(),
        99
    );
}

#[test]
fn duplicate_names_need_ids_and_folders_are_reserved_without_overwriting() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ProjectRegistry::open(temp.path()).unwrap();
    fs::create_dir_all(temp.path().join("projects/ring3")).unwrap();
    fs::write(
        temp.path().join("projects/ring3/crash-evidence"),
        b"retained",
    )
    .unwrap();
    let first = registry.create("ring3", None).unwrap();
    let second = registry.create("ring3", None).unwrap();
    assert_eq!(first.store_path.file_name().unwrap(), "ring3-2");
    assert_eq!(second.store_path.file_name().unwrap(), "ring3-3");
    assert!(
        registry
            .get("ring3")
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
    assert_eq!(registry.get(&first.id).unwrap(), first);
    assert_eq!(
        fs::read(temp.path().join("projects/ring3/crash-evidence")).unwrap(),
        b"retained"
    );
    assert_eq!(
        registry
            .create("CON", None)
            .unwrap()
            .store_path
            .file_name()
            .unwrap(),
        "project-con"
    );
    for invalid in [
        "",
        "..\n",
        " trailing",
        "trailing ",
        "legacy",
        "Legacy shared store",
    ] {
        assert!(registry.create(invalid, None).is_err());
    }
}

#[test]
fn legacy_store_remains_explicit_and_measurements_are_not_redistributed() {
    let temp = tempfile::tempdir().unwrap();
    Store::open(temp.path())
        .unwrap()
        .save_run(&recording("old mixed history"))
        .unwrap();
    let registry = ProjectRegistry::open(temp.path()).unwrap();
    let legacy = registry.get("legacy").unwrap();
    assert!(legacy.legacy);
    assert_eq!(registry.list().unwrap(), vec![legacy.clone()]);
    let connection = Connection::open(temp.path().join("hunin.sqlite")).unwrap();
    assert!(
        !connection
            .query_row::<bool, _, _>(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='project_identity')",
                [],
                |row| row.get(0)
            )
            .unwrap()
    );
    let workspace = temp.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let project = registry.resolve_or_create(&workspace).unwrap();
    assert!(
        registry
            .open_store(&project)
            .unwrap()
            .runs(10, 0)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        registry
            .open_store(&legacy)
            .unwrap()
            .run("same-recording-id")
            .unwrap()
            .name,
        "old mixed history"
    );
}

#[test]
fn moved_store_can_be_registered_without_changing_identity_or_run_history() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ProjectRegistry::open(temp.path()).unwrap();
    let project = registry.create("ring3", None).unwrap();
    registry
        .open_store(&project)
        .unwrap()
        .save_run(&recording("original"))
        .unwrap();
    let moved = temp.path().join("renamed-folder");
    fs::rename(&project.store_path, &moved).unwrap();
    assert!(registry.open_store(&project).is_err());
    let repaired = registry.register("ring3", &moved, None).unwrap();
    assert_eq!(repaired.id, project.id);
    assert_eq!(repaired.store_path, moved.canonicalize().unwrap());
    let run = registry
        .open_store(&repaired)
        .unwrap()
        .run("same-recording-id")
        .unwrap();
    assert_eq!(run.cwd, "/original/measured/checkout");
    assert_eq!(run.source, json!({"revision":"original"}));
}

#[test]
fn registering_an_existing_store_is_explicit_and_refuses_another_live_copy() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ProjectRegistry::open(&temp.path().join("registry")).unwrap();
    let old_root = temp.path().join("old-store");
    Store::open(&old_root)
        .unwrap()
        .save_run(&recording("old"))
        .unwrap();
    let adopted = registry
        .register("chosen-project", &old_root, None)
        .unwrap();
    assert_eq!(
        registry
            .open_store(&adopted)
            .unwrap()
            .run("same-recording-id")
            .unwrap()
            .name,
        "old"
    );
    let copy = temp.path().join("copy");
    fs::create_dir(&copy).unwrap();
    fs::copy(old_root.join("hunin.sqlite"), copy.join("hunin.sqlite")).unwrap();
    assert!(
        registry
            .register("chosen-project", &copy, None)
            .unwrap_err()
            .to_string()
            .contains("another copy")
    );
    assert_eq!(registry.get(&adopted.id).unwrap(), adopted);
}

#[test]
fn concurrent_default_resolution_publishes_one_project_and_store() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ProjectRegistry::open(&temp.path().join("registry")).unwrap();
    let workspace = temp.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let barrier = Arc::new(Barrier::new(4));
    let handles = (0..4)
        .map(|_| {
            let registry = registry.clone();
            let workspace = workspace.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                registry.resolve_or_create(&workspace).unwrap()
            })
        })
        .collect::<Vec<_>>();
    let projects = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert!(projects.iter().all(|project| project == &projects[0]));
    assert_eq!(registry.list().unwrap().len(), 1);
    assert_eq!(
        fs::read_dir(registry.root().join("projects"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn registered_workspace_owns_descendants_and_nested_registration_wins() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ProjectRegistry::open(&temp.path().join("registry")).unwrap();
    let outer = temp.path().join("outer");
    let nested = outer.join("nested");
    fs::create_dir_all(nested.join("src")).unwrap();
    let first = registry.create("outer", Some(&outer)).unwrap();
    assert_eq!(
        registry
            .resolve_or_create(&nested.join("src"))
            .unwrap()
            .workspace_roots,
        first.workspace_roots
    );
    let second = registry.create("nested", None).unwrap();
    registry
        .register("nested", &second.store_path, Some(&nested))
        .unwrap();
    assert_eq!(
        registry
            .for_workspace(&nested.join("src"))
            .unwrap()
            .unwrap()
            .id,
        second.id
    );
    assert_eq!(
        registry.for_workspace(&outer).unwrap().unwrap().id,
        first.id
    );
}

fn git(root: &Path, arguments: &[&str]) {
    let result = std::process::Command::new("git")
        .current_dir(root)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn automatic_parent_does_not_absorb_new_git_repositories_or_their_recordings() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ProjectRegistry::open(&temp.path().join("registry")).unwrap();
    let parent = temp.path().join("work");
    fs::create_dir(&parent).unwrap();
    let incidental = registry.resolve_or_create(&parent).unwrap();
    let mut projects = Vec::new();
    for name in ["alpha", "beta"] {
        let root = parent.join(name);
        fs::create_dir_all(root.join(".git")).unwrap();
        let project = registry.resolve_or_create(&root).unwrap();
        assert_ne!(project.id, incidental.id);
        registry
            .open_store(&project)
            .unwrap()
            .save_run(&recording(name))
            .unwrap();
        projects.push(project);
    }
    assert_ne!(projects[0].id, projects[1].id);
    for (project, name) in projects.iter().zip(["alpha", "beta"]) {
        assert_eq!(
            registry
                .open_store(project)
                .unwrap()
                .run("same-recording-id")
                .unwrap()
                .name,
            name
        );
    }
    assert!(
        registry
            .open_store(&incidental)
            .unwrap()
            .runs(10, 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn explicit_workspace_container_can_group_new_git_repositories() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ProjectRegistry::open(&temp.path().join("registry")).unwrap();
    let parent = temp.path().join("intentional-group");
    fs::create_dir(&parent).unwrap();
    let group = registry.create("group", Some(&parent)).unwrap();
    for name in ["alpha", "beta"] {
        let root = parent.join(name);
        fs::create_dir_all(root.join(".git")).unwrap();
        assert_eq!(registry.resolve_or_create(&root).unwrap().id, group.id);
    }
}

#[test]
fn registry_upgrade_preserves_existing_bindings_without_guessing_group_intent() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("registry");
    let registry = ProjectRegistry::open(&root).unwrap();
    let workspace = temp.path().join("old-workspace");
    fs::create_dir(&workspace).unwrap();
    let original = registry.create("original", Some(&workspace)).unwrap();
    registry
        .open_store(&original)
        .unwrap()
        .save_run(&recording("retained"))
        .unwrap();
    let old = Connection::open(root.join("registry.sqlite")).unwrap();
    old.execute_batch(
        "ALTER TABLE workspace_aliases DROP COLUMN git_descendants; PRAGMA user_version=1;",
    )
    .unwrap();
    drop(old);
    let upgraded = ProjectRegistry::open(&root).unwrap();
    assert_eq!(
        upgraded.for_workspace(&workspace).unwrap().unwrap(),
        original
    );
    assert_eq!(
        upgraded
            .open_store(&original)
            .unwrap()
            .run("same-recording-id")
            .unwrap()
            .name,
        "retained"
    );
    let new_repo = workspace.join("new-repo");
    fs::create_dir_all(new_repo.join(".git")).unwrap();
    assert_ne!(
        upgraded.resolve_or_create(&new_repo).unwrap().id,
        original.id
    );
    upgraded
        .register("original", &original.store_path, Some(&workspace))
        .unwrap();
    let grouped_repo = workspace.join("explicitly-grouped");
    fs::create_dir_all(grouped_repo.join(".git")).unwrap();
    assert_eq!(
        upgraded.resolve_or_create(&grouped_repo).unwrap().id,
        original.id
    );
}

#[test]
fn linked_git_worktrees_share_one_project_while_another_repo_does_not() {
    let temp = tempfile::tempdir().unwrap();
    let registry = ProjectRegistry::open(&temp.path().join("registry")).unwrap();
    let main = temp.path().join("repo");
    fs::create_dir(&main).unwrap();
    git(&main, &["init", "--initial-branch=main"]);
    git(
        &main,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
    fs::create_dir(main.join("src")).unwrap();
    let first = registry.resolve_or_create(&main.join("src")).unwrap();
    let container = temp.path().join("container");
    fs::create_dir(&container).unwrap();
    let other_project = registry.create("container", Some(&container)).unwrap();
    let linked = container.join("linked");
    git(
        &main,
        &["worktree", "add", "-b", "feature", linked.to_str().unwrap()],
    );
    let linked_project = registry.resolve_or_create(&linked).unwrap();
    assert_eq!(first.id, linked_project.id);
    assert_ne!(linked_project.id, other_project.id);
    assert_eq!(linked_project.workspace_roots.len(), 2);
    let separate = temp.path().join("separate");
    fs::create_dir(&separate).unwrap();
    git(&separate, &["init", "--initial-branch=main"]);
    assert_ne!(registry.resolve_or_create(&separate).unwrap().id, first.id);
    assert_eq!(registry.list().unwrap().len(), 3);
}

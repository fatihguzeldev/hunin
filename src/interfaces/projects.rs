//! Process-owned project sessions; request selection never changes another tab.
use crate::{
    operations::Operations,
    projects::{Project, ProjectRegistry},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, watch};

#[derive(Clone)]
pub struct ProjectSessions {
    source: Source,
    state: Arc<Mutex<SessionState>>,
}

#[derive(Clone)]
enum Source {
    Direct(Box<Operations>),
    Registry {
        registry: ProjectRegistry,
        selected_project_id: Option<String>,
    },
}

#[derive(Default)]
struct SessionState {
    closing: bool,
    opened: HashMap<String, Session>,
}

type Session = watch::Receiver<Option<std::result::Result<(Project, Operations), String>>>;

impl ProjectSessions {
    pub fn direct(operations: Operations) -> Self {
        Self {
            source: Source::Direct(Box::new(operations)),
            state: Arc::new(Mutex::new(SessionState::default())),
        }
    }

    pub fn registry(registry: ProjectRegistry, selected_project_id: Option<String>) -> Self {
        Self {
            source: Source::Registry {
                registry,
                selected_project_id,
            },
            state: Arc::new(Mutex::new(SessionState::default())),
        }
    }

    pub async fn catalog(&self) -> Result<Value> {
        match &self.source {
            Source::Direct(operations) => Ok(json!({
                "projects":[], "selected_project_id":null, "direct_store":true,
                "storage_scope":operations.engine.store().root().to_string_lossy(),
            })),
            Source::Registry {
                registry,
                selected_project_id,
            } => {
                let registry = registry.clone();
                let projects = tokio::task::spawn_blocking(move || registry.list()).await??;
                Ok(
                    json!({"projects":projects,"selected_project_id":selected_project_id,"direct_store":false,"storage_scope":null}),
                )
            }
        }
    }

    pub async fn operations(&self, project_id: Option<&str>) -> Result<Operations> {
        let mut state = self.state.lock().await;
        ensure!(!state.closing, "Hunin is shutting down");
        match &self.source {
            Source::Direct(operations) => {
                ensure!(
                    project_id.is_none(),
                    "This server uses a direct store; project_id must be omitted"
                );
                Ok(operations.as_ref().clone())
            }
            Source::Registry { registry, .. } => {
                let id = project_id.context("Select a project and include its project_id")?;
                let mut session = state
                    .opened
                    .entry(id.to_owned())
                    .or_insert_with(|| {
                        let (sender, receiver) = watch::channel(None);
                        let registry = registry.clone();
                        let id = id.to_owned();
                        // The server owns initialization even when the requesting
                        // tab disconnects. Admission and shutdown use the same lock.
                        tokio::spawn(async move {
                            let result = tokio::task::spawn_blocking(move || {
                                let project = registry.get(&id)?;
                                ensure!(project.id == id, "project_id must be a stable project ID");
                                let operations =
                                    Operations::try_new(registry.open_store(&project)?)?;
                                Ok::<_, anyhow::Error>((project, operations))
                            })
                            .await;
                            let result = match result {
                                Ok(result) => result.map_err(|error| format!("{error:#}")),
                                Err(error) => {
                                    Err(format!("Project initialization failed: {error}"))
                                }
                            };
                            sender.send_replace(Some(result));
                        });
                        receiver
                    })
                    .clone();
                drop(state);
                let result = session
                    .wait_for(Option::is_some)
                    .await
                    .context("Project initialization stopped")?
                    .clone()
                    .unwrap();
                if result.is_err() {
                    // A missing/moved store can be repaired and retried without
                    // restarting the server; never remove a replacement entry.
                    let mut state = self.state.lock().await;
                    if state
                        .opened
                        .get(id)
                        .is_some_and(|current| current.same_channel(&session))
                    {
                        state.opened.remove(id);
                    }
                }
                let (project, operations) = result.map_err(anyhow::Error::msg)?;
                let registry = registry.clone();
                tokio::task::spawn_blocking(move || registry.verify_store(&project))
                    .await?
                    .context("Project store identity changed or became unavailable; restore the registered store, or restart Hunin after repairing its registration")?;
                Ok(operations)
            }
        }
    }

    pub async fn shutdown(&self) -> Result<()> {
        let sessions: Vec<Session> = {
            let mut state = self.state.lock().await;
            state.closing = true;
            state.opened.values().cloned().collect()
        };
        if let Source::Direct(operations) = &self.source {
            return operations.shutdown().await;
        }
        let mut failures = Vec::new();
        for mut session in sessions {
            let initialized = match session.wait_for(Option::is_some).await {
                Ok(initialized) => initialized.clone().unwrap(),
                Err(error) => {
                    failures.push(format!("Project initialization stopped: {error}"));
                    continue;
                }
            };
            // A failed opener has no engine or admitted durable jobs to close.
            if let Ok((_, operations)) = initialized
                && let Err(error) = operations.shutdown().await
            {
                failures.push(format!("{error:#}"));
            }
        }
        ensure!(
            failures.is_empty(),
            "Project shutdown failed: {}",
            failures.join("; ")
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::runs::Run, storage::Store};

    fn recording(store: &Store, name: &str) {
        store
            .save_run(&Run {
                id: "same-recording-id".into(),
                name: name.into(),
                status: "completed".into(),
                command: vec![],
                cwd: ".".into(),
                runtime: "none".into(),
                started_at_us: 1,
                finished_at_us: Some(2),
                exit_code: Some(0),
                diagnostics: vec![],
                source: json!({}),
                correctness: json!({}),
            })
            .unwrap();
    }

    #[tokio::test]
    async fn two_project_requests_keep_same_named_recordings_in_their_own_stores() {
        let root = tempfile::tempdir().unwrap();
        let registry = ProjectRegistry::open(root.path()).unwrap();
        let first = registry.create("first", None).unwrap();
        let second = registry.create("second", None).unwrap();
        recording(&registry.open_store(&first).unwrap(), "first recording");
        recording(&registry.open_store(&second).unwrap(), "second recording");
        let sessions = ProjectSessions::registry(registry, Some(first.id.clone()));
        assert!(
            sessions.operations(None).await.is_err(),
            "a launch default cannot select another tab's store"
        );
        assert!(sessions.operations(Some("missing")).await.is_err());
        assert!(
            sessions.operations(Some("first")).await.is_err(),
            "request IDs are not display names"
        );
        let a = sessions.operations(Some(&first.id)).await.unwrap();
        let b = sessions.operations(Some(&second.id)).await.unwrap();
        let (a, b) = tokio::join!(
            a.call("runs.get", json!({"run_id":"same-recording-id"})),
            b.call("runs.get", json!({"run_id":"same-recording-id"}))
        );
        assert_eq!(a.unwrap()["run"]["name"], "first recording");
        assert_eq!(b.unwrap()["run"]["name"], "second recording");
        let first_again = sessions.operations(Some(&first.id)).await.unwrap();
        assert_eq!(
            first_again.call("runs.list", json!({})).await.unwrap()["runs"][0]["name"],
            "first recording"
        );
        sessions.shutdown().await.unwrap();
        assert!(sessions.operations(Some(&second.id)).await.is_err());
    }

    #[tokio::test]
    async fn catalog_lists_a_missing_store_without_creating_or_opening_it() {
        let root = tempfile::tempdir().unwrap();
        let registry = ProjectRegistry::open(root.path()).unwrap();
        let project = registry.create("offline", None).unwrap();
        std::fs::remove_file(project.store_path.join("hunin.sqlite")).unwrap();
        let sessions = ProjectSessions::registry(registry, None);
        let catalog = sessions.catalog().await.unwrap();
        assert_eq!(catalog["projects"][0]["id"], project.id);
        assert_eq!(catalog["selected_project_id"], Value::Null);
        assert_eq!(catalog["direct_store"], false);
        assert!(sessions.operations(Some(&project.id)).await.is_err());
        assert!(!project.store_path.join("hunin.sqlite").exists());
    }

    #[tokio::test]
    async fn direct_store_has_its_own_browser_scope_and_rejects_project_requests() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let scope = store.root().to_string_lossy().into_owned();
        let sessions = ProjectSessions::direct(Operations::new(store));
        let catalog = sessions.catalog().await.unwrap();
        assert_eq!(catalog["direct_store"], true);
        assert_eq!(catalog["storage_scope"], scope);
        assert!(sessions.operations(Some("legacy")).await.is_err());
        assert!(sessions.operations(None).await.is_ok());
        sessions.shutdown().await.unwrap();
        assert!(sessions.operations(None).await.is_err());
    }

    #[tokio::test]
    async fn cached_session_rejects_a_store_replaced_by_another_project() {
        let root = tempfile::tempdir().unwrap();
        let registry = ProjectRegistry::open(root.path()).unwrap();
        let first = registry.create("first", None).unwrap();
        let second = registry.create("second", None).unwrap();
        let sessions = ProjectSessions::registry(registry, None);
        sessions.operations(Some(&first.id)).await.unwrap();
        let retained = root.path().join("retained-first");
        std::fs::rename(&first.store_path, &retained).unwrap();
        std::fs::rename(&second.store_path, &first.store_path).unwrap();
        assert!(sessions.operations(Some(&first.id)).await.is_err());
        std::fs::rename(&first.store_path, &second.store_path).unwrap();
        std::fs::rename(retained, &first.store_path).unwrap();
        sessions.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn abandoned_first_request_keeps_initialization_owned_through_shutdown() {
        use std::time::Duration;
        let root = tempfile::tempdir().unwrap();
        let registry = ProjectRegistry::open(root.path()).unwrap();
        let project = registry.create("blocked", None).unwrap();
        let connection =
            rusqlite::Connection::open(project.store_path.join("hunin.sqlite")).unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        let sessions = ProjectSessions::registry(registry, None);
        let request = {
            let sessions = sessions.clone();
            let id = project.id.clone();
            tokio::spawn(async move { sessions.operations(Some(&id)).await })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if sessions.state.lock().await.opened.contains_key(&project.id) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        request.abort();
        assert!(request.await.err().unwrap().is_cancelled());
        let mut shutdown = {
            let sessions = sessions.clone();
            tokio::spawn(async move { sessions.shutdown().await })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut shutdown)
                .await
                .is_err(),
            "shutdown must observe the admitted initializer even after its caller leaves"
        );
        connection.execute_batch("COMMIT").unwrap();
        tokio::time::timeout(Duration::from_secs(5), shutdown)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let session = sessions.state.lock().await.opened[&project.id].clone();
        let operations = session
            .borrow()
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .1
            .clone();
        assert!(
            operations.call("runs.list", json!({})).await.is_err(),
            "the initialized worker pools must have been shut down"
        );
    }
}

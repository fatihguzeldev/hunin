use super::{Project, identity, workspace::Workspace};
use crate::storage::Store;
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const REGISTRY_VERSION: u32 = 1;
const LEGACY_ID: &str = "legacy";

#[derive(Clone)]
pub struct ProjectRegistry {
    root: PathBuf,
}

impl ProjectRegistry {
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root)?;
        let registry = Self {
            root: root.canonicalize()?,
        };
        let mut connection = registry.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: u32 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        ensure!(
            version <= REGISTRY_VERSION,
            "project registry requires a newer Hunin"
        );
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                store_path TEXT NOT NULL UNIQUE
            );
            CREATE INDEX IF NOT EXISTS project_name ON projects(name);
            CREATE TABLE IF NOT EXISTS workspace_aliases (
                path TEXT NOT NULL,
                kind TEXT NOT NULL CHECK(kind IN ('workspace','git_common')),
                project_id TEXT NOT NULL REFERENCES projects(id),
                PRIMARY KEY(path,kind)
            );
            CREATE INDEX IF NOT EXISTS project_workspace ON workspace_aliases(project_id,kind);",
        )?;
        tx.pragma_update(None, "user_version", REGISTRY_VERSION)?;
        tx.commit()?;
        Ok(registry)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads only registry metadata. Missing or corrupt stores remain visible.
    pub fn list(&self) -> Result<Vec<Project>> {
        let connection = self.connection()?;
        let mut statement =
            connection.prepare("SELECT id,name,store_path FROM projects ORDER BY name,id")?;
        let mut projects = statement
            .query_map([], project_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for project in &mut projects {
            project.workspace_roots = workspace_roots(&connection, &project.id)?;
        }
        if let Some(legacy) = self.legacy(&connection)? {
            projects.push(legacy);
        }
        Ok(projects)
    }

    /// Stable ID wins; duplicate display names require the stable ID.
    pub fn get(&self, selector: &str) -> Result<Project> {
        let connection = self.connection()?;
        if let Some(project) = by_id(&connection, selector)? {
            return Ok(project);
        }
        let legacy = self.legacy(&connection)?;
        if selector == LEGACY_ID {
            return legacy.context("legacy shared store is unavailable");
        }
        let mut statement = connection
            .prepare("SELECT id,name,store_path FROM projects WHERE name=?1 ORDER BY id LIMIT 2")?;
        let mut projects = statement
            .query_map([selector], project_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if let Some(legacy) = legacy.filter(|project| project.name == selector) {
            projects.push(legacy);
        }
        ensure!(
            projects.len() <= 1,
            "project name is ambiguous; select its project ID"
        );
        let mut project = projects
            .pop()
            .with_context(|| format!("project not found: {selector}"))?;
        project.workspace_roots = workspace_roots(&connection, &project.id)?;
        Ok(project)
    }

    pub fn create(&self, name: &str, workspace: Option<&Path>) -> Result<Project> {
        validate_name(name)?;
        let workspace = workspace.map(Workspace::discover).transpose()?;
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(workspace) = &workspace {
            workspace_available(&tx, None, workspace)?;
        }
        let project = self.create_in(&tx, name, workspace.as_ref())?;
        tx.commit()?;
        Ok(project)
    }

    /// Explicitly adopts an old store or repairs discovery after moving a store.
    /// Its recordings and artifacts are never copied, reassigned, or deleted.
    pub fn register(
        &self,
        name: &str,
        store_root: &Path,
        workspace: Option<&Path>,
    ) -> Result<Project> {
        validate_name(name)?;
        let store_path = store_root
            .canonicalize()
            .context("existing project store is unavailable")?;
        ensure!(
            store_path.join("hunin.sqlite").is_file(),
            "registration requires an existing hunin.sqlite"
        );
        let workspace = workspace.map(Workspace::discover).transpose()?;
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing_identity = identity::read(&store_path)?;
        let registered_id: Option<String> = tx
            .query_row(
                "SELECT id FROM projects WHERE store_path=?1",
                [path_text(&store_path)?],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(registered_id) = registered_id {
            ensure!(
                existing_identity
                    .as_ref()
                    .is_some_and(|identity| identity.id == registered_id),
                "registered store identity is missing or changed; refusing adoption"
            );
        }
        if let Some(workspace) = &workspace {
            workspace_available(
                &tx,
                existing_identity
                    .as_ref()
                    .map(|identity| identity.id.as_str()),
                workspace,
            )?;
        }
        let identity = match existing_identity {
            Some(identity) => identity,
            None => {
                // Validate the recording schema before adding identity metadata.
                Store::open(&store_path)?;
                identity::assign(&store_path, &uuid::Uuid::new_v4().to_string(), name)?
            }
        };
        ensure!(
            identity.name == name,
            "store already has project name {}; register using that name",
            identity.name
        );
        if let Some(previous) = by_id(&tx, &identity.id)? {
            ensure!(
                previous.store_path == store_path
                    || !previous.store_path.join("hunin.sqlite").exists(),
                "project is already registered at {}; refuse to select another copy",
                previous.store_path.display()
            );
        }
        tx.execute(
            "INSERT INTO projects(id,name,store_path) VALUES(?1,?2,?3)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name,store_path=excluded.store_path",
            params![identity.id, identity.name, path_text(&store_path)?],
        )?;
        if let Some(workspace) = &workspace {
            bind_workspace(&tx, &identity.id, workspace)?;
        }
        let project = by_id(&tx, &identity.id)?.context("registered project disappeared")?;
        tx.commit()?;
        Ok(project)
    }

    pub fn for_workspace(&self, path: &Path) -> Result<Option<Project>> {
        let workspace = Workspace::discover(path)?;
        workspace_project(&self.connection()?, &workspace)
    }

    /// Default CLI use has one serialization point for lookup and creation.
    pub fn resolve_or_create(&self, path: &Path) -> Result<Project> {
        let workspace = Workspace::discover(path)?;
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let project = match workspace_project(&tx, &workspace)? {
            Some(project) => {
                // Ordinary non-Git cwd changes are not new workspace roots.
                if workspace.common_dir.is_some() {
                    bind_workspace(&tx, &project.id, &workspace)?;
                }
                by_id(&tx, &project.id)?.context("resolved project disappeared")?
            }
            None => self.create_in(&tx, &workspace.default_name(), Some(&workspace))?,
        };
        tx.commit()?;
        Ok(project)
    }

    /// Reject absent/replaced stores before migrations or operations can touch it.
    pub fn verify_store(&self, project: &Project) -> Result<()> {
        if project.legacy {
            ensure!(
                project.id == LEGACY_ID && project.store_path == self.root,
                "invalid legacy store selection"
            );
            ensure!(
                identity::read(&project.store_path)?.is_none(),
                "legacy store was adopted as a project; select that project"
            );
        } else {
            let identity = identity::read(&project.store_path)?
                .context("selected store has no project identity")?;
            ensure!(
                identity.id == project.id,
                "project store identity mismatch; registry path points to another project"
            );
        }
        Ok(())
    }

    pub fn open_store(&self, project: &Project) -> Result<Store> {
        self.verify_store(project)?;
        if project.legacy {
            Store::open_legacy(&project.store_path)
        } else {
            Store::open_project(&project.store_path, &project.id)
        }
    }

    fn create_in(
        &self,
        tx: &Transaction<'_>,
        name: &str,
        workspace: Option<&Workspace>,
    ) -> Result<Project> {
        let parent = self.root.join("projects");
        fs::create_dir_all(&parent)?;
        let slug = folder_name(name);
        // create_dir reserves the folder even when a previous process crashed
        // before publishing its registry row. Such stores are never overwritten.
        let mut suffix = 1_u64;
        let store_path = loop {
            let folder = if suffix == 1 {
                slug.clone()
            } else {
                format!("{slug}-{suffix}")
            };
            let path = parent.join(folder);
            match fs::create_dir(&path) {
                Ok(()) => break path.canonicalize()?,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    suffix = suffix
                        .checked_add(1)
                        .context("too many project folder collisions")?
                }
                Err(error) => return Err(error.into()),
            }
        };
        let store = Store::open(&store_path)?;
        let identity = identity::assign(store.root(), &uuid::Uuid::new_v4().to_string(), name)?;
        tx.execute(
            "INSERT INTO projects(id,name,store_path) VALUES(?1,?2,?3)",
            params![identity.id, identity.name, path_text(&store_path)?],
        )?;
        if let Some(workspace) = workspace {
            bind_workspace(tx, &identity.id, workspace)?;
        }
        by_id(tx, &identity.id)?.context("created project disappeared")
    }

    fn legacy(&self, connection: &Connection) -> Result<Option<Project>> {
        if !self.root.join("hunin.sqlite").is_file() {
            return Ok(None);
        }
        let registered: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE store_path=?1)",
            [path_text(&self.root)?],
            |row| row.get(0),
        )?;
        Ok((!registered).then(|| Project {
            id: LEGACY_ID.into(),
            name: "Legacy shared store".into(),
            store_path: self.root.clone(),
            workspace_roots: vec![],
            legacy: true,
        }))
    }

    fn connection(&self) -> Result<Connection> {
        let connection = Connection::open(self.root.join("registry.sqlite"))?;
        connection.busy_timeout(Duration::from_secs(10))?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        Ok(connection)
    }
}

fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.trim().is_empty() && name == name.trim() && name.len() <= 200,
        "project name must contain 1–200 bytes without surrounding whitespace"
    );
    ensure!(
        !name.chars().any(char::is_control),
        "project name cannot contain control characters"
    );
    ensure!(
        name != LEGACY_ID && name != "Legacy shared store",
        "project name is reserved for the legacy shared store"
    );
    Ok(())
}

fn folder_name(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    // Prefix Windows device names rather than producing an unusable folder.
    let reserved = matches!(slug, "con" | "prn" | "aux" | "nul")
        || slug
            .strip_prefix("com")
            .or_else(|| slug.strip_prefix("lpt"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'));
    if slug.is_empty() {
        "project".into()
    } else if reserved {
        format!("project-{slug}")
    } else {
        slug.into()
    }
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .context("project and workspace paths must be valid Unicode")
}

fn project_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        store_path: PathBuf::from(row.get::<_, String>(2)?),
        workspace_roots: vec![],
        legacy: false,
    })
}

fn by_id(connection: &Connection, id: &str) -> Result<Option<Project>> {
    let mut project = connection
        .query_row(
            "SELECT id,name,store_path FROM projects WHERE id=?1",
            [id],
            project_row,
        )
        .optional()?;
    if let Some(project) = &mut project {
        project.workspace_roots = workspace_roots(connection, id)?;
    }
    Ok(project)
}

fn workspace_roots(connection: &Connection, id: &str) -> Result<Vec<PathBuf>> {
    let mut statement = connection.prepare(
        "SELECT path FROM workspace_aliases WHERE project_id=?1 AND kind='workspace' ORDER BY path",
    )?;
    Ok(statement
        .query_map([id], |row| row.get::<_, String>(0).map(PathBuf::from))?
        .collect::<rusqlite::Result<_>>()?)
}

fn workspace_project(connection: &Connection, workspace: &Workspace) -> Result<Option<Project>> {
    let exact: Option<String> = connection
        .query_row(
            "SELECT project_id FROM workspace_aliases WHERE path=?1 AND kind='workspace'",
            [path_text(&workspace.root)?],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = exact {
        return by_id(connection, &id);
    }
    if let Some(common_dir) = &workspace.common_dir {
        let id: Option<String> = connection
            .query_row(
                "SELECT project_id FROM workspace_aliases WHERE path=?1 AND kind='git_common'",
                [path_text(common_dir)?],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            return by_id(connection, &id);
        }
    }
    // A linked checkout belongs to its known Git project even when somebody
    // places it below a different registered workspace container.
    for path in workspace.root.ancestors().skip(1) {
        let id: Option<String> = connection
            .query_row(
                "SELECT project_id FROM workspace_aliases WHERE path=?1 AND kind='workspace'",
                [path_text(path)?],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            return by_id(connection, &id);
        }
    }
    Ok(None)
}

fn bind_workspace(tx: &Transaction<'_>, id: &str, workspace: &Workspace) -> Result<()> {
    workspace_available(tx, Some(id), workspace)?;
    for (kind, path) in std::iter::once(("workspace", &workspace.root)).chain(
        workspace
            .common_dir
            .as_ref()
            .map(|path| ("git_common", path)),
    ) {
        tx.execute(
            "INSERT OR IGNORE INTO workspace_aliases(path,kind,project_id) VALUES(?1,?2,?3)",
            params![path_text(path)?, kind, id],
        )?;
    }
    Ok(())
}

fn workspace_available(
    connection: &Connection,
    id: Option<&str>,
    workspace: &Workspace,
) -> Result<()> {
    for (kind, path) in std::iter::once(("workspace", &workspace.root)).chain(
        workspace
            .common_dir
            .as_ref()
            .map(|path| ("git_common", path)),
    ) {
        let previous: Option<String> = connection
            .query_row(
                "SELECT project_id FROM workspace_aliases WHERE path=?1 AND kind=?2",
                params![path_text(path)?, kind],
                |row| row.get(0),
            )
            .optional()?;
        if previous
            .as_deref()
            .is_some_and(|previous| Some(previous) != id)
        {
            bail!("workspace already belongs to another project");
        }
    }
    Ok(())
}

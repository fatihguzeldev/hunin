use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type Result<T> = std::result::Result<T, Error>;
pub const MAX_VALUE_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("reference belongs to another project")]
    ForeignProject,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("interrupted: {0}")]
    Interrupted(String),
    #[error("source is closed")]
    Closed,
    #[error("resource limit: {0}")]
    Limit(String),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Reference {
    pub project: String,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TargetRef {
    pub project: String,
    pub run: String,
    pub id: String,
}

#[derive(Clone)]
pub struct Project {
    inner: Arc<Inner>,
}

struct Inner {
    id: String,
    directory: PathBuf,
    connection: Mutex<Connection>,
}

impl std::fmt::Debug for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Project")
            .field("id", &self.id())
            .field("directory", &self.directory())
            .finish_non_exhaustive()
    }
}

const APPLICATION_ID: i64 = 0x4855_4e49;
const DATABASE_VERSION: i64 = 1;

impl Project {
    pub fn new_id() -> Result<String> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(|error| Error::Interrupted(error.to_string()))?;
        Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    pub fn create(data_root: impl AsRef<Path>, id: &str) -> Result<Self> {
        Self::load(data_root.as_ref(), id, true)
    }

    pub fn open(data_root: impl AsRef<Path>, id: &str) -> Result<Self> {
        Self::load(data_root.as_ref(), id, false)
    }

    fn load(root: &Path, id: &str, create: bool) -> Result<Self> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(Error::Invalid(
                "project id must contain 1..=128 ascii letters, digits, '-' or '_'".into(),
            ));
        }
        if rusqlite::version() != "3.54.1" {
            return Err(Error::Invalid(format!(
                "sqlite 3.54.1 is required, found {}",
                rusqlite::version()
            )));
        }
        let root = if root.is_absolute() {
            root.to_path_buf()
        } else {
            std::env::current_dir()?.join(root)
        };
        let mut directory = root.join("projects").join(id);
        let database = directory.join("project.sqlite");
        for path in [&directory, &database] {
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(Error::Invalid(
                        "project data cannot be a symbolic link".into(),
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        if create {
            fs::create_dir_all(&directory)?;
        }
        if !create && !database.is_file() {
            return Err(Error::NotFound(format!("project {id}")));
        }
        directory = fs::canonicalize(directory)?;
        let database = directory.join("project.sqlite");
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | if create {
                OpenFlags::SQLITE_OPEN_CREATE
            } else {
                OpenFlags::empty()
            };
        let mut connection = Connection::open_with_flags(&database, flags)?;
        connection.busy_timeout(Duration::from_secs(2))?;
        let app: i64 = connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let tables: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;
        if tables == 0 && create && app == 0 && version == 0 {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(SCHEMA)?;
            transaction.execute("INSERT INTO project_info(project_id,revision,metadata) VALUES (?1,0,'{}') ON CONFLICT(project_id) DO NOTHING", [id])?;
            let stored: String =
                transaction
                    .query_row("SELECT project_id FROM project_info", [], |row| row.get(0))?;
            if stored != id {
                return Err(Error::ForeignProject);
            }
            transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
            transaction.pragma_update(None, "user_version", DATABASE_VERSION)?;
            transaction.commit()?;
        } else {
            if app != APPLICATION_ID || version != DATABASE_VERSION {
                return Err(Error::Invalid(
                    "unknown project database or incompatible schema".into(),
                ));
            }
            let stored: String =
                connection
                    .query_row("SELECT project_id FROM project_info", [], |row| row.get(0))?;
            if stored != id {
                return Err(Error::ForeignProject);
            }
        }
        configure(&connection)?;
        let temporary = directory.join("tmp");
        if fs::symlink_metadata(&temporary).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(Error::Invalid(
                "project temporary directory cannot be a symbolic link".into(),
            ));
        }
        fs::create_dir_all(&temporary)?;
        Ok(Self {
            inner: Arc::new(Inner {
                id: id.to_owned(),
                directory,
                connection: Mutex::new(connection),
            }),
        })
    }

    pub fn id(&self) -> &str {
        &self.inner.id
    }
    pub fn directory(&self) -> &Path {
        &self.inner.directory
    }
    pub fn reference(&self, id: impl Into<String>) -> Reference {
        Reference {
            project: self.id().to_owned(),
            id: id.into(),
        }
    }

    pub(crate) fn check_reference(&self, reference: &Reference) -> Result<()> {
        if reference.project != self.id() {
            return Err(Error::ForeignProject);
        }
        validate_id(&reference.id)
    }

    pub(crate) fn check_run(&self, run: &Reference) -> Result<()> {
        self.check_reference(run)?;
        self.with_connection(|connection| require_id(connection, "runs", &run.id))
    }

    pub(crate) fn with_connection<T>(
        &self,
        action: impl FnOnce(&mut Connection) -> Result<T>,
    ) -> Result<T> {
        let mut connection = self
            .inner
            .connection
            .lock()
            .map_err(|_| Error::Interrupted("project connection was poisoned".into()))?;
        action(&mut connection)
    }

    pub(crate) fn transaction<T>(
        &self,
        action: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let result = action(&transaction)?;
            transaction.commit()?;
            Ok(result)
        })
    }

    pub fn revision(&self) -> Result<u64> {
        self.with_connection(|connection| revision(connection))
    }

    pub fn metadata(&self) -> Result<Value> {
        self.with_connection(|connection| {
            let body: String =
                connection.query_row("SELECT metadata FROM project_info", [], |row| row.get(0))?;
            Ok(serde_json::from_str(&body)?)
        })
    }

    pub fn set_metadata(&self, metadata: Value) -> Result<u64> {
        let body = encode_bounded(&metadata)?;
        self.transaction(|transaction| {
            transaction.execute("UPDATE project_info SET metadata=?1", [&body])?;
            bump_revision(transaction)
        })
    }

    pub fn create_investigation(&self, id: &str, name: &str, metadata: Value) -> Result<Reference> {
        validate_id(id)?;
        if name.is_empty() || name.len() > 4096 {
            return Err(Error::Invalid(
                "investigation name must contain 1..=4096 bytes".into(),
            ));
        }
        let body = encode_bounded(&metadata)?;
        self.transaction(|transaction| {
            let existing: Option<(String, String)> = transaction
                .query_row(
                    "SELECT name,metadata FROM investigations WHERE id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some(existing) = existing {
                if existing != (name.to_owned(), body.clone()) {
                    return Err(Error::Conflict(format!("investigation {id}")));
                }
            } else {
                transaction.execute(
                    "INSERT INTO investigations(id,name,metadata) VALUES (?1,?2,?3)",
                    params![id, name, body],
                )?;
                bump_revision(transaction)?;
            }
            Ok(self.reference(id))
        })
    }

    pub fn create_run(
        &self,
        investigation: &Reference,
        id: &str,
        conditions: Value,
    ) -> Result<Reference> {
        self.check_reference(investigation)?;
        validate_id(id)?;
        let body = encode_bounded(&conditions)?;
        self.transaction(|transaction| {
            require_id(transaction, "investigations", &investigation.id)?;
            let existing: Option<(String, String)> = transaction
                .query_row(
                    "SELECT investigation,conditions FROM runs WHERE id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some(existing) = existing {
                if existing != (investigation.id.clone(), body.clone()) {
                    return Err(Error::Conflict(format!("run {id}")));
                }
            } else {
                transaction.execute(
                    "INSERT INTO runs(id,investigation,conditions) VALUES (?1,?2,?3)",
                    params![id, investigation.id, body],
                )?;
                bump_revision(transaction)?;
            }
            Ok(self.reference(id))
        })
    }

    pub fn register_target(
        &self,
        run: &Reference,
        id: &str,
        identity: Value,
        owned: bool,
    ) -> Result<TargetRef> {
        self.check_reference(run)?;
        validate_id(id)?;
        let body = encode_bounded(&identity)?;
        self.transaction(|transaction| {
            require_id(transaction, "runs", &run.id)?;
            let existing: Option<(String, bool)> = transaction
                .query_row(
                    "SELECT identity,owned FROM targets WHERE run=?1 AND id=?2",
                    params![run.id, id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some(existing) = existing {
                if existing != (body.clone(), owned) {
                    return Err(Error::Conflict(format!("target {id}")));
                }
            } else {
                ensure_run_active(transaction, &run.id)?;
                transaction.execute(
                    "INSERT INTO targets(run,id,identity,owned) VALUES (?1,?2,?3,?4)",
                    params![run.id, id, body, owned],
                )?;
                bump_revision(transaction)?;
            }
            Ok(TargetRef { project: self.id().to_owned(), run: run.id.clone(), id: id.to_owned() })
        })
    }

    pub fn run_conditions(&self, run: &Reference) -> Result<Value> {
        self.check_reference(run)?;
        self.with_connection(|connection| {
            let body: String = connection
                .query_row(
                    "SELECT conditions FROM runs WHERE id=?1",
                    [&run.id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| Error::NotFound(format!("run {}", run.id)))?;
            Ok(serde_json::from_str(&body)?)
        })
    }
}

fn configure(connection: &Connection) -> Result<()> {
    let valid: bool = connection.query_row("SELECT sqlite_compileoption_used('TEMP_STORE=3') AND sqlite_compileoption_used('STMTJRNL_SPILL=-1')",[],|row|row.get(0))?;
    if !valid {
        return Err(Error::Invalid(
            "sqlite needs TEMP_STORE=3 and STMTJRNL_SPILL=-1 build flags".into(),
        ));
    }
    connection.execute_batch("PRAGMA temp_store=MEMORY; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA trusted_schema=OFF; PRAGMA cache_size=-8192;")?;
    let mode: String = connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
    if mode != "wal" {
        return Err(Error::Invalid(
            "project database requires local WAL support".into(),
        ));
    }
    #[cfg(target_os = "macos")]
    connection.execute_batch("PRAGMA fullfsync=ON;")?;
    connection.set_limit(
        rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
        2 * 1024 * 1024,
    )?;
    Ok(())
}

pub(crate) fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err(Error::Invalid(
            "identifier must contain 1..=256 non-control bytes".into(),
        ));
    }
    Ok(())
}

pub(crate) fn canonical(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(array) => Value::Array(array.into_iter().map(canonical).collect()),
        value => value,
    }
}

pub(crate) fn encode_bounded(value: &impl Serialize) -> Result<String> {
    let body = serde_json::to_string(&canonical(serde_json::to_value(value)?))?;
    if body.len() > MAX_VALUE_BYTES {
        return Err(Error::Limit("structured value exceeds 1 mib".into()));
    }
    Ok(body)
}

pub(crate) fn revision(connection: &Connection) -> Result<u64> {
    let value: i64 =
        connection.query_row("SELECT revision FROM project_info", [], |row| row.get(0))?;
    u64::try_from(value).map_err(|_| Error::Invalid("negative project revision".into()))
}

pub(crate) fn bump_revision(transaction: &Transaction<'_>) -> Result<u64> {
    if transaction.execute(
        "UPDATE project_info SET revision=revision+1 WHERE revision<9223372036854775807",
        [],
    )? != 1
    {
        return Err(Error::Limit("project revision exhausted".into()));
    }
    revision(transaction)
}

pub(crate) fn require_id(connection: &Connection, table: &str, id: &str) -> Result<()> {
    let sql = match table {
        "runs" => "SELECT 1 FROM runs WHERE id=?1",
        "investigations" => "SELECT 1 FROM investigations WHERE id=?1",
        _ => return Err(Error::Invalid("invalid identifier owner".into())),
    };
    if connection
        .query_row(sql, [id], |row| row.get::<_, i64>(0))
        .optional()?
        .is_none()
    {
        return Err(Error::NotFound(format!("{table}/{id}")));
    }
    Ok(())
}

pub(crate) fn ensure_run_active(connection: &Connection, run: &str) -> Result<()> {
    require_id(connection, "runs", run)?;
    let terminal: Option<Option<String>> = connection.query_row("SELECT terminal FROM control_scopes WHERE run_id=?1", [run], |row| row.get(0)).optional()?;
    if terminal.flatten().is_some() { return Err(Error::Conflict("run is terminal; new work needs a new run".into())); }
    Ok(())
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS project_info(project_id TEXT PRIMARY KEY,revision INTEGER NOT NULL CHECK(revision>=0),metadata TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS investigations(id TEXT PRIMARY KEY,name TEXT NOT NULL,metadata TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS runs(id TEXT PRIMARY KEY,investigation TEXT NOT NULL REFERENCES investigations(id),conditions TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS control_scopes(run_id TEXT PRIMARY KEY REFERENCES runs(id),generation INTEGER NOT NULL,terminal TEXT) STRICT;
CREATE TABLE IF NOT EXISTS control_requests(caller TEXT NOT NULL,id TEXT NOT NULL,run_id TEXT NOT NULL REFERENCES runs(id),action TEXT NOT NULL,arguments TEXT NOT NULL,generation INTEGER NOT NULL,state TEXT NOT NULL,result TEXT,PRIMARY KEY(caller,id)) STRICT;
CREATE TABLE IF NOT EXISTS targets(run TEXT NOT NULL REFERENCES runs(id),id TEXT NOT NULL,identity TEXT NOT NULL,owned INTEGER NOT NULL CHECK(owned IN(0,1)),PRIMARY KEY(run,id)) STRICT;
CREATE TABLE IF NOT EXISTS sources(run TEXT NOT NULL REFERENCES runs(id),id TEXT NOT NULL,definition TEXT NOT NULL,closed INTEGER NOT NULL DEFAULT 0 CHECK(closed IN(0,1)),outcome TEXT,cutoff INTEGER,PRIMARY KEY(run,id)) STRICT;
CREATE TABLE IF NOT EXISTS source_schemas(run TEXT NOT NULL,source TEXT NOT NULL,name TEXT NOT NULL,version INTEGER NOT NULL,body TEXT NOT NULL,PRIMARY KEY(run,source,name,version),FOREIGN KEY(run,source) REFERENCES sources(run,id)) STRICT;
CREATE TABLE IF NOT EXISTS source_events(seq INTEGER PRIMARY KEY,run TEXT NOT NULL,source TEXT NOT NULL,kind TEXT NOT NULL,id TEXT NOT NULL,body TEXT NOT NULL,revision INTEGER NOT NULL,UNIQUE(run,source,kind,id),FOREIGN KEY(run,source) REFERENCES sources(run,id)) STRICT;
CREATE TABLE IF NOT EXISTS delivery_batches(run TEXT NOT NULL,source TEXT NOT NULL,id TEXT NOT NULL,digest BLOB NOT NULL,record_count INTEGER NOT NULL,revision INTEGER NOT NULL,PRIMARY KEY(run,source,id),FOREIGN KEY(run,source) REFERENCES sources(run,id)) STRICT;
CREATE TABLE IF NOT EXISTS records(seq INTEGER PRIMARY KEY,run TEXT NOT NULL,source TEXT NOT NULL,id TEXT NOT NULL,schema_name TEXT NOT NULL,schema_version INTEGER NOT NULL,subject TEXT NOT NULL,clock TEXT,start INTEGER,end INTEGER,uncertainty TEXT,function TEXT,location TEXT,operation TEXT,body TEXT NOT NULL,body_bytes INTEGER NOT NULL,revision INTEGER NOT NULL,UNIQUE(run,source,id),FOREIGN KEY(run,source) REFERENCES sources(run,id)) STRICT;
CREATE INDEX IF NOT EXISTS records_source ON records(run,source,seq);
CREATE INDEX IF NOT EXISTS records_function ON records(run,function,seq);
CREATE INDEX IF NOT EXISTS records_operation ON records(run,operation,seq);
CREATE INDEX IF NOT EXISTS records_location ON records(run,location,seq);
CREATE INDEX IF NOT EXISTS records_time ON records(run,clock,start,seq);
CREATE TABLE IF NOT EXISTS record_fields(record INTEGER NOT NULL REFERENCES records(seq),name TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(record,name)) STRICT;
CREATE INDEX IF NOT EXISTS field_lookup ON record_fields(name,value,record);
CREATE TABLE IF NOT EXISTS contexts(seq INTEGER PRIMARY KEY,run TEXT NOT NULL REFERENCES runs(id),id TEXT NOT NULL,subject TEXT NOT NULL,origin TEXT NOT NULL,body TEXT NOT NULL,corrects TEXT,revision INTEGER NOT NULL,UNIQUE(run,id)) STRICT;
CREATE TABLE IF NOT EXISTS relationships(seq INTEGER PRIMARY KEY,run TEXT NOT NULL REFERENCES runs(id),id TEXT NOT NULL,from_run TEXT NOT NULL,from_source TEXT NOT NULL,from_id TEXT NOT NULL,to_run TEXT NOT NULL,to_source TEXT NOT NULL,to_id TEXT NOT NULL,kind TEXT NOT NULL,origin TEXT NOT NULL,body TEXT NOT NULL,corrects TEXT,revision INTEGER NOT NULL,UNIQUE(run,id)) STRICT;
CREATE INDEX IF NOT EXISTS relations_forward ON relationships(from_run,from_source,from_id,revision,seq);
CREATE INDEX IF NOT EXISTS relations_reverse ON relationships(to_run,to_source,to_id,revision,seq);
"#;

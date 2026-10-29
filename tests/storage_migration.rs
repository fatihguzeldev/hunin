use hunin::{model::runs::Run, storage::Store};
use rusqlite::{Connection, params};
use serde_json::json;
use sha2::{Digest, Sha256};

#[test]
fn a_schema_one_copy_migrates_owned_history_and_shared_bytes_without_changing_observations() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("artifacts")).unwrap();
    let bytes = b"legacy raw evidence";
    let sha = format!("{:x}", Sha256::digest(bytes));
    std::fs::write(root.path().join("artifacts").join(&sha), bytes).unwrap();
    let run = json!({"id":"legacy","name":"legacy copy","status":"legacy-partial","command":[],"cwd":"/removed","runtime":"imported","started_at_us":1,"finished_at_us":2,"exit_code":null,"diagnostics":[],"source":null,"correctness":null});
    let record = json!({"schema_version":1,"kind":"event","id":"retained","run_id":"legacy","producer_id":"old","pid":1,"tid":"","timestamp_us":1,"basis":"observed","name":"original","attributes":{"extension":[null,7]}});
    let db = Connection::open(root.path().join("hunin.sqlite")).unwrap();
    db.execute_batch("PRAGMA user_version=1;
        CREATE TABLE runs(id TEXT PRIMARY KEY,started_at_us INTEGER NOT NULL,manifest TEXT NOT NULL);
        CREATE TABLE result_sets(id TEXT PRIMARY KEY,run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,input_identity TEXT NOT NULL,UNIQUE(run_id,input_identity));
        CREATE TABLE observations(seq INTEGER PRIMARY KEY,run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,id TEXT NOT NULL,kind TEXT NOT NULL,timestamp_us INTEGER NOT NULL,result_set_id TEXT NOT NULL REFERENCES result_sets(id) ON DELETE CASCADE,record TEXT NOT NULL,UNIQUE(run_id,id));
        CREATE TABLE artifacts(run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,id TEXT NOT NULL,metadata TEXT NOT NULL,PRIMARY KEY(run_id,id));
        CREATE TABLE documents(namespace TEXT NOT NULL,id TEXT NOT NULL,payload TEXT NOT NULL,updated_at_us INTEGER NOT NULL,PRIMARY KEY(namespace,id));").unwrap();
    db.execute("INSERT INTO runs VALUES('legacy',1,?1)", [run.to_string()])
        .unwrap();
    db.execute(
        "INSERT INTO result_sets VALUES('old-result','legacy','old-input')",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO observations VALUES(1,'legacy','retained','event',1,'old-result',?1)",
        [record.to_string()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO artifacts VALUES('legacy','raw',?1)",
        [
            json!({"id":"raw","name":"raw.profile","format":"raw","size":bytes.len(),"sha256":sha})
                .to_string(),
        ],
    )
    .unwrap();
    for namespace in ["analysis-results", "profile-inputs", "cancellations"] {
        db.execute(
            "INSERT INTO documents VALUES(?1,?2,?3,1)",
            params![
                namespace,
                namespace,
                json!({"run_id":"legacy","status":"accepted","result_set_id":"old-result"})
                    .to_string()
            ],
        )
        .unwrap();
    }
    drop(db);
    let store = Store::open(root.path()).unwrap();
    assert_eq!(
        serde_json::to_value(store.run("legacy").unwrap()).unwrap(),
        run
    );
    assert_eq!(store.records("legacy").unwrap(), vec![record]);
    assert_eq!(
        std::fs::read(store.artifact_path("legacy", "raw").unwrap()).unwrap(),
        bytes
    );
    let db = Connection::open(root.path().join("hunin.sqlite")).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        db.query_row(
            "SELECT source_key FROM result_sets WHERE id='old-result'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "old-input"
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM documents WHERE owner_run_id='legacy'",
            [],
            |r| r.get::<_, usize>(0)
        )
        .unwrap(),
        3
    );
    drop(db);
    drop(store);
    let reopened = Store::open(root.path()).unwrap();
    let _: Run = reopened.run("legacy").unwrap();
    reopened.remove_run("legacy").unwrap();
    assert!(reopened.documents("profile-inputs").unwrap().is_empty());
}

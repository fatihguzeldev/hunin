CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY,
    started_at_us INTEGER NOT NULL,
    manifest TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS result_sets (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    input_identity TEXT NOT NULL,
    UNIQUE(run_id,input_identity)
);
CREATE TABLE IF NOT EXISTS observations (
    seq INTEGER PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    kind TEXT NOT NULL,
    timestamp_us INTEGER NOT NULL,
    result_set_id TEXT NOT NULL REFERENCES result_sets(id) ON DELETE CASCADE,
    record TEXT NOT NULL,
    UNIQUE(run_id,id)
);
CREATE INDEX IF NOT EXISTS observation_timeline ON observations(run_id,timestamp_us,seq);
CREATE INDEX IF NOT EXISTS observation_kind ON observations(run_id,kind);
CREATE TABLE IF NOT EXISTS artifacts (
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    metadata TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY(run_id,id)
);

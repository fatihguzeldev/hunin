CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY,
    started_at_us INTEGER NOT NULL,
    manifest TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS run_count_request ON runs(json_extract(manifest,'$.source.call_counting.request_key')) WHERE json_valid(manifest);
CREATE INDEX IF NOT EXISTS run_count_parent ON runs(json_extract(manifest,'$.source.call_counting.parent_run_id'),started_at_us DESC,id DESC) WHERE json_valid(manifest);
CREATE TABLE IF NOT EXISTS result_sets (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    input_identity TEXT NOT NULL,
    source_key TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1,
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
CREATE TABLE IF NOT EXISTS result_set_dependencies (
    result_set_id TEXT NOT NULL REFERENCES result_sets(id) ON DELETE CASCADE,
    input_result_set_id TEXT NOT NULL REFERENCES result_sets(id) ON DELETE CASCADE,
    PRIMARY KEY(result_set_id,input_result_set_id)
);
CREATE INDEX IF NOT EXISTS result_dependency_input ON result_set_dependencies(input_result_set_id);
CREATE INDEX IF NOT EXISTS observation_timeline ON observations(run_id,timestamp_us,seq);
CREATE INDEX IF NOT EXISTS observation_kind ON observations(run_id,kind);
CREATE TABLE IF NOT EXISTS artifacts (
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    metadata TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY(run_id,id)
);
CREATE TABLE IF NOT EXISTS documents (
    namespace TEXT NOT NULL,
    id TEXT NOT NULL,
    payload TEXT NOT NULL,
    updated_at_us INTEGER NOT NULL,
    owner_run_id TEXT REFERENCES runs(id) ON DELETE CASCADE,
    PRIMARY KEY(namespace,id)
);

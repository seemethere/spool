CREATE TABLE IF NOT EXISTS launcher_session_data (
    agent_run_id TEXT PRIMARY KEY NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    launcher_kind TEXT NOT NULL,
    session_id TEXT,
    model TEXT,
    provider TEXT,
    started_at TEXT,
    finished_at TEXT,
    final_status TEXT,
    transcript_path TEXT,
    raw_json TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_launcher_session_data_launcher_kind
    ON launcher_session_data(launcher_kind);

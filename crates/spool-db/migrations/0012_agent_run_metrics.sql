CREATE TABLE IF NOT EXISTS agent_run_metrics (
    agent_run_id TEXT PRIMARY KEY NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    duration_ms INTEGER,
    launcher_kind TEXT NOT NULL,
    final_status TEXT,
    exit_code INTEGER,
    timed_out INTEGER,
    unattended_question_detected INTEGER,
    blocking_ui_detected INTEGER,
    transcript_path TEXT,
    transcript_byte_size INTEGER,
    transcript_jsonl_event_count INTEGER,
    input_tokens INTEGER,
    output_tokens INTEGER,
    total_tokens INTEGER,
    warnings_json TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_agent_run_metrics_launcher_kind
    ON agent_run_metrics(launcher_kind);

CREATE INDEX IF NOT EXISTS idx_agent_run_metrics_final_status
    ON agent_run_metrics(final_status);

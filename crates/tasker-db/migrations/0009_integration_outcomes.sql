CREATE TABLE IF NOT EXISTS integration_outcomes (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    agent_run_id TEXT REFERENCES agent_runs(id),
    outcome_kind TEXT NOT NULL CHECK (outcome_kind IN ('success', 'no_changes', 'work_change_failure', 'operational_failure')),
    final_commit TEXT,
    pre_merge_head TEXT,
    message TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_integration_outcomes_task_created
ON integration_outcomes(task_id, created_at);

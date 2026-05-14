ALTER TABLE task_queues ADD COLUMN queue_concurrency_limit INTEGER CHECK (queue_concurrency_limit IS NULL OR queue_concurrency_limit > 0);

CREATE TABLE IF NOT EXISTS agent_runs (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    task_queue_id TEXT NOT NULL REFERENCES task_queues(id) ON DELETE CASCADE,
    worker_actor_kind TEXT NOT NULL,
    worker_actor_id TEXT NOT NULL,
    worker_actor_display_name TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    launcher_kind TEXT NOT NULL,
    lease_expires_at TEXT NOT NULL,
    last_heartbeat_at TEXT,
    outcome TEXT CHECK (outcome IS NULL OR outcome IN ('completed', 'failed', 'canceled', 'expired')),
    failure_reason TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_runs_one_active_per_task
ON agent_runs(task_id)
WHERE outcome IS NULL;

CREATE INDEX IF NOT EXISTS idx_agent_runs_queue_active
ON agent_runs(task_queue_id, outcome, lease_expires_at);

CREATE INDEX IF NOT EXISTS idx_agent_runs_lease_expiry
ON agent_runs(outcome, lease_expires_at);

CREATE TABLE IF NOT EXISTS task_retry_holds (
    task_id TEXT PRIMARY KEY NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    agent_run_id TEXT REFERENCES agent_runs(id) ON DELETE SET NULL,
    hold_until TEXT NOT NULL,
    reason TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_task_retry_holds_until
ON task_retry_holds(hold_until);

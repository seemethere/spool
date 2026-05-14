CREATE TABLE IF NOT EXISTS task_conflict_hints (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    target TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(task_id, position),
    UNIQUE(task_id, target)
);

CREATE INDEX IF NOT EXISTS idx_task_conflict_hints_target
ON task_conflict_hints(target);

CREATE TABLE IF NOT EXISTS task_queues (
    id TEXT PRIMARY KEY NOT NULL,
    key TEXT UNIQUE NOT NULL,
    name TEXT NOT NULL,
    delivery_backend TEXT NOT NULL DEFAULT 'local_worktree',
    managed_source_repository TEXT NOT NULL,
    main_branch TEXT NOT NULL,
    worktree_root TEXT NOT NULL,
    branch_template TEXT NOT NULL,
    done_worktree_retention INTEGER NOT NULL DEFAULT 0 CHECK (done_worktree_retention IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS audit_events (
    id TEXT PRIMARY KEY NOT NULL,
    actor_kind TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    actor_display_name TEXT NOT NULL,
    event_type TEXT NOT NULL,
    subject_type TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_audit_events_subject
ON audit_events(subject_type, subject_id, created_at);

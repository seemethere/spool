ALTER TABLE task_queues ADD COLUMN next_task_sequence INTEGER NOT NULL DEFAULT 1;

CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY NOT NULL,
    task_queue_id TEXT NOT NULL REFERENCES task_queues(id),
    identifier TEXT UNIQUE NOT NULL,
    sequence INTEGER NOT NULL,
    title TEXT NOT NULL,
    brief TEXT NOT NULL,
    priority TEXT NOT NULL CHECK (priority IN ('urgent', 'high', 'normal', 'low')),
    state TEXT NOT NULL CHECK (state IN ('backlog', 'ready', 'in_progress', 'human_review', 'rework', 'integrating', 'done', 'canceled')),
    review_required INTEGER NOT NULL DEFAULT 0 CHECK (review_required IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(task_queue_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_tasks_queue_state
ON tasks(task_queue_id, state, priority, created_at, identifier);

CREATE TABLE IF NOT EXISTS acceptance_criteria (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    description TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'satisfied', 'waived')),
    waiver_reason TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(task_id, position)
);

CREATE TABLE IF NOT EXISTS validation_items (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    description TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'passed', 'failed', 'waived')),
    waiver_reason TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(task_id, position)
);

CREATE TABLE IF NOT EXISTS task_tags (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    PRIMARY KEY(task_id, tag)
);

CREATE TABLE IF NOT EXISTS workpad_notes (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL UNIQUE REFERENCES tasks(id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS workpad_revisions (
    id TEXT PRIMARY KEY NOT NULL,
    workpad_note_id TEXT NOT NULL REFERENCES workpad_notes(id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

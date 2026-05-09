CREATE TABLE IF NOT EXISTS task_relationships (
    id TEXT PRIMARY KEY NOT NULL,
    source_task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    target_task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    relationship_kind TEXT NOT NULL CHECK (relationship_kind IN ('parent_child', 'blocks')),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(source_task_id, target_task_id, relationship_kind)
);

CREATE INDEX IF NOT EXISTS idx_task_relationships_source
ON task_relationships(source_task_id, relationship_kind);

CREATE INDEX IF NOT EXISTS idx_task_relationships_target
ON task_relationships(target_task_id, relationship_kind);

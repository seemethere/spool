CREATE TABLE integration_outcomes_new (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    agent_run_id TEXT REFERENCES agent_runs(id),
    outcome_kind TEXT NOT NULL CHECK (outcome_kind IN ('success', 'no_changes', 'work_change_failure', 'operational_failure')),
    final_commit TEXT,
    pre_merge_head TEXT,
    message TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    retryable INTEGER NOT NULL DEFAULT 0 CHECK (retryable IN (0, 1)),
    retry_attempt INTEGER CHECK (retry_attempt IS NULL OR retry_attempt > 0),
    next_retry_at TEXT,
    reason_code TEXT CHECK (
        reason_code IS NULL OR reason_code IN (
            'success',
            'no_changes',
            'uncommitted_local_worktree',
            'stale_validated_base_commit',
            'auto_refresh_success',
            'auto_refresh_conflict',
            'auto_refresh_validation_failed',
            'auto_refresh_declined_missing_validation',
            'task_branch_missing_main',
            'dirty_managed_source_repository',
            'repo_operation_lock_held',
            'merge_conflict',
            'cleanup_failure',
            'unknown_operational_failure',
            'unknown_work_change_failure',
            'unknown_legacy'
        )
    )
);

INSERT INTO integration_outcomes_new (
    id, task_id, agent_run_id, outcome_kind, final_commit, pre_merge_head, message,
    created_at, retryable, retry_attempt, next_retry_at, reason_code
)
SELECT
    id, task_id, agent_run_id, outcome_kind, final_commit, pre_merge_head, message,
    created_at, retryable, retry_attempt, next_retry_at, reason_code
FROM integration_outcomes;

DROP TABLE integration_outcomes;
ALTER TABLE integration_outcomes_new RENAME TO integration_outcomes;

CREATE INDEX IF NOT EXISTS idx_integration_outcomes_task_created
ON integration_outcomes(task_id, created_at);

CREATE INDEX IF NOT EXISTS idx_integration_outcomes_retry
ON integration_outcomes(task_id, retryable, next_retry_at);

CREATE INDEX IF NOT EXISTS idx_integration_outcomes_reason_code
ON integration_outcomes(reason_code, created_at);

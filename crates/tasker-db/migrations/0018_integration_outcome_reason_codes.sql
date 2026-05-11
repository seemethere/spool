ALTER TABLE integration_outcomes ADD COLUMN reason_code TEXT CHECK (
    reason_code IS NULL OR reason_code IN (
        'success',
        'no_changes',
        'uncommitted_local_worktree',
        'stale_validated_base_commit',
        'task_branch_missing_main',
        'dirty_managed_source_repository',
        'repo_operation_lock_held',
        'merge_conflict',
        'cleanup_failure',
        'unknown_operational_failure',
        'unknown_work_change_failure',
        'unknown_legacy'
    )
);

CREATE INDEX IF NOT EXISTS idx_integration_outcomes_reason_code
ON integration_outcomes(reason_code, created_at);

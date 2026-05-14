ALTER TABLE agent_runs ADD COLUMN failure_reason_code TEXT CHECK (
    failure_reason_code IS NULL OR failure_reason_code IN (
        'agent_run_failed',
        'local_worktree_setup_failed',
        'dirty_managed_source_repository',
        'repo_operation_lock_held',
        'migration_incompatible',
        'stale_validation_base',
        'launcher_start_failed',
        'launcher_rpc_io_failed',
        'launcher_exited',
        'launcher_timeout',
        'unattended_question',
        'agent_gated_integration_failed',
        'operator_failed',
        'claim_lease_expired',
        'task_canceled'
    )
);

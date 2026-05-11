# Integration Outcome reason codes

Integration Outcomes store a structured `reason_code` alongside the existing human-readable `message`. Older rows may have a null `reason_code`; status, monitor, and telemetry classify those rows as `unknown_legacy` when grouping operator-facing delivery pain.

## Codes

- `success` — Local Worktree Delivery created a Final Commit and no cleanup problem was reported.
- `no_changes` — the Task Branch had no repository changes to deliver.
- `uncommitted_local_worktree` — the Local Worktree had uncommitted changes and the Task should return to Rework.
- `stale_validated_base_commit` — the Task Branch did not include current Main Branch and the recorded Validated Base Commit no longer matched Main Branch.
- `task_branch_missing_main` — the Task Branch did not include current Main Branch and no current Validated Base Commit allowed integration.
- `dirty_managed_source_repository` — the Managed Source Repository had unexpected uncommitted changes.
- `repo_operation_lock_held` — a Git or Tasker Managed Source Repository operation lock blocked delivery.
- `merge_conflict` — squash merge failed and the Task should return to Rework.
- `cleanup_failure` — integration/no-change completion succeeded but Local Worktree or Task Branch cleanup needs operator repair.
- `unknown_operational_failure` — retryable or operator-facing delivery failure without a more specific operational classification.
- `unknown_work_change_failure` — work-change failure without a more specific classification.
- `unknown_legacy` — older Integration Outcome row without a stored reason code.

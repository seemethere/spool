//! Tasker persistence layer.
//!
//! The crate keeps the external `tasker_db::...` API stable while organizing
//! persistence by Tasker domain area so Worker Agents can inspect and edit a
//! narrower file for future slices. SQLite migrations stay in `migrations/` and
//! are loaded by the connection module.

mod audit;
mod auth;
mod connection;
mod integration;
mod lifecycle;
mod metrics;
mod models;
mod queues;
mod requirements;
mod review;
mod runs;
mod status;
mod task_draft;
mod tasks;
mod transitions;
mod validation;
mod workpad;

pub use audit::list_task_audit_events;
pub use auth::{authenticate_api_token, ensure_local_api_token, get_api_token};
pub use connection::{
    check_migration_compatibility, connect, pending_migration_versions, run_migrations, sqlite_url,
    LOCAL_TOKEN_NAME,
};
pub use integration::{is_valid_integration_outcome_reason_code, record_integration_outcome};
pub use metrics::{
    compute_agent_run_metrics, get_agent_run, get_agent_run_detail, get_agent_run_metrics,
    get_latest_agent_run_detail_for_task, get_launcher_session_data, refresh_agent_run_metrics,
    upsert_launcher_session_data, CURRENT_AGENT_RUN_METRICS_DERIVATION_VERSION,
};
pub use models::*;
pub use queues::{
    create_task_queue, get_task_queue, list_audit_events, list_task_queue_audit_events,
    list_task_queues, update_task_queue_concurrency_limit,
};
pub use requirements::{
    record_task_validated_base_commit, update_acceptance_criterion_status,
    update_validation_item_status,
};
pub use review::record_review_decision;
pub use runs::{claim_next, finish_run, heartbeat_run, operator_fail_run, retry_task};
pub use status::{
    active_agent_runs_for_status, active_retry_holds_for_status, due_integration_retries,
    integration_retries_for_status, merge_queue_tasks, status_by_queue_and_state,
    task_conflict_groups_for_status, tasks_for_status_by_states,
};
pub use task_draft::{
    create_delegated_root_task, validate_delegation_task_draft, DelegationTaskDraft,
};
pub use tasks::{
    create_child_task, create_task, get_task_context_bundle, get_task_detail, refine_backlog_task,
    upsert_task_link,
};
pub use transitions::transition_task_state;
pub use validation::validate_create_task;
pub use workpad::{count_workpad_revisions, update_workpad_note};

pub(crate) use audit::append_audit_event_in_tx;
pub(crate) use connection::with_sqlite_write_retry;
pub(crate) use lifecycle::{agent_run_select_sql, expire_stale_agent_runs};
pub(crate) use tasks::unresolved_blocking_task_count;
pub(crate) use validation::*;

#[cfg(test)]
mod tests;

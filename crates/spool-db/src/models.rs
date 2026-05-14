#![allow(unused_imports)]

use crate::*;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    FromRow, SqlitePool,
};
use std::{fs, future::Future, path::Path, time::Duration};
use tokio::time::sleep;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Actor {
    pub kind: String,
    pub id: String,
    pub display_name: String,
}

impl Actor {
    pub fn operator(display_name: impl Into<String>) -> Self {
        let display_name = display_name.into();
        Self {
            kind: "operator".to_string(),
            id: display_name.clone(),
            display_name,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateTaskQueue {
    pub key: String,
    pub name: String,
    pub managed_source_repository: String,
    pub main_branch: String,
    pub worktree_root: String,
    pub branch_template: String,
    pub done_worktree_retention: bool,
    pub queue_concurrency_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskQueue {
    pub id: String,
    pub key: String,
    pub name: String,
    pub delivery_backend: String,
    pub managed_source_repository: String,
    pub main_branch: String,
    pub worktree_root: String,
    pub branch_template: String,
    pub done_worktree_retention: bool,
    pub queue_concurrency_limit: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateQueueConcurrencyLimit {
    pub queue_concurrency_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct AuditEvent {
    pub id: String,
    pub actor_kind: String,
    pub actor_id: String,
    pub actor_display_name: String,
    pub event_type: String,
    pub subject_type: String,
    pub subject_id: String,
    pub payload_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateTask {
    pub queue_key: String,
    pub title: String,
    pub brief: String,
    pub priority: String,
    pub state: String,
    pub review_required: bool,
    pub acceptance_criteria: Vec<String>,
    pub validation_items: Vec<String>,
    pub tags: Vec<String>,
    #[serde(default)]
    pub conflict_hints: Vec<String>,
    #[serde(default)]
    pub blocking_task_identifiers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateChildTask {
    pub title: String,
    pub brief: String,
    pub priority: String,
    pub state: String,
    pub review_required: bool,
    pub acceptance_criteria: Vec<String>,
    pub validation_items: Vec<String>,
    pub tags: Vec<String>,
    #[serde(default)]
    pub conflict_hints: Vec<String>,
    pub blocks_parent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefineBacklogTask {
    pub title: Option<String>,
    pub brief: Option<String>,
    pub priority: Option<String>,
    pub target_state: Option<String>,
    pub review_required: Option<bool>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub validation_items: Vec<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub conflict_hints: Option<Vec<String>>,
    #[serde(default)]
    pub blocking_task_identifiers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub task_queue_id: String,
    pub task_queue_key: String,
    pub identifier: String,
    pub sequence: i64,
    pub title: String,
    pub brief: String,
    pub priority: String,
    pub state: String,
    pub review_required: bool,
    pub validated_base_commit: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct AcceptanceCriterion {
    pub id: String,
    pub task_id: String,
    pub position: i64,
    pub description: String,
    pub status: String,
    pub waiver_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct ValidationItem {
    pub id: String,
    pub task_id: String,
    pub position: i64,
    pub description: String,
    pub status: String,
    pub waiver_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct WorkpadNote {
    pub id: String,
    pub task_id: String,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskLink {
    pub id: String,
    pub task_id: String,
    pub kind: String,
    pub target: String,
    pub label: Option<String>,
    pub is_primary: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskConflictHint {
    pub id: String,
    pub task_id: String,
    pub position: i64,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskConflictOverlap {
    pub target: String,
    pub task_identifier: String,
    pub title: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct BlockingTaskSummary {
    pub identifier: String,
    pub title: String,
    pub state: String,
    pub resolved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskConflictGroup {
    pub queue_key: String,
    pub target: String,
    pub task_count: i64,
    pub tasks: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpsertTaskLink {
    pub kind: String,
    pub target: String,
    pub label: Option<String>,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskDetail {
    pub task: Task,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub validation_items: Vec<ValidationItem>,
    pub tags: Vec<String>,
    pub workpad_note: Option<WorkpadNote>,
    pub task_links: Vec<TaskLink>,
    pub conflict_hints: Vec<TaskConflictHint>,
    pub conflict_overlaps: Vec<TaskConflictOverlap>,
    pub blocking_tasks: Vec<BlockingTaskSummary>,
    pub blocked_tasks: Vec<BlockingTaskSummary>,
    pub latest_rework_reason_code: Option<String>,
    pub latest_rework_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskContextBundle {
    pub task: TaskDetail,
    pub queue: TaskContextQueue,
    pub local_workflow: TaskLocalWorkflowContext,
    pub advisory_hints: TaskContextAdvisoryHints,
    pub agent_runs: Vec<TaskContextAgentRun>,
    pub latest_failure: Option<TaskContextRunFailure>,
    pub latest_integration_outcome: Option<TaskContextIntegrationOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskContextAdvisoryHints {
    pub note: String,
    pub task_conflict_hints: Vec<TaskConflictHint>,
    pub likely_files_or_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskContextQueue {
    pub key: String,
    pub name: String,
    pub delivery_backend: String,
    pub main_branch: String,
    pub managed_source_repository: String,
    pub worktree_root: String,
    pub branch_template: String,
    pub queue_concurrency_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskLocalWorkflowContext {
    pub local_worktree: Option<String>,
    pub task_branch: Option<String>,
    pub main_branch: String,
    pub managed_source_repository: String,
    pub worktree_root: String,
    pub branch_template: String,
    pub delivery_backend: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskContextAgentRun {
    pub id: String,
    pub worker_actor_kind: String,
    pub worker_actor_id: String,
    pub worker_actor_display_name: String,
    pub worker_id: String,
    pub launcher_kind: String,
    pub lease_expires_at: String,
    pub last_heartbeat_at: Option<String>,
    pub outcome: Option<String>,
    pub failure_reason: Option<String>,
    pub failure_reason_code: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub is_active: bool,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub final_status: Option<String>,
    pub duration_ms: Option<i64>,
    pub tool_call_count: Option<i64>,
    pub tool_error_count: Option<i64>,
    pub repeated_failed_tool_attempt_count: Option<i64>,
    pub repeated_read_count: Option<i64>,
    pub repeated_tasker_context_fetch_count: Option<i64>,
    pub total_tokens: Option<i64>,
    pub max_context_tokens: Option<i64>,
    pub efficiency_hints_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskContextRunFailure {
    pub agent_run_id: String,
    pub outcome: String,
    pub failure_reason: Option<String>,
    pub failure_reason_code: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskContextIntegrationOutcome {
    pub id: String,
    pub agent_run_id: Option<String>,
    pub outcome_kind: String,
    pub reason_code: Option<String>,
    pub final_commit: Option<String>,
    pub pre_merge_head: Option<String>,
    pub message: Option<String>,
    pub retryable: bool,
    pub retry_attempt: Option<i64>,
    pub next_retry_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct QueueStatus {
    pub queue_key: String,
    pub queue_name: String,
    pub queue_concurrency_limit: Option<i64>,
    pub state: String,
    pub task_count: i64,
    pub ready_tasks: i64,
    pub integrating_tasks: i64,
    pub active_agent_runs: i64,
    pub active_integrating_agent_runs: i64,
    pub active_retry_holds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct ActiveAgentRunStatus {
    pub queue_key: String,
    pub task_identifier: String,
    pub task_title: String,
    pub task_state: String,
    pub agent_run_id: String,
    pub launcher_kind: String,
    pub worker_id: String,
    pub lease_expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskStatusSummary {
    pub queue_key: String,
    pub identifier: String,
    pub title: String,
    pub state: String,
    pub priority: String,
    pub local_worktree: Option<String>,
    pub task_branch: Option<String>,
    pub main_branch: String,
    pub latest_rework_reason_code: Option<String>,
    pub latest_rework_reason: Option<String>,
    pub unresolved_blocking_task_count: i64,
    pub blocking_task_identifiers: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct ActiveRetryHoldStatus {
    pub queue_key: String,
    pub task_identifier: String,
    pub hold_until: String,
    pub reason: String,
    pub failure_reason_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct MergeQueueTask {
    pub queue_key: String,
    pub task_identifier: String,
    pub title: String,
    pub task_branch: Option<String>,
    pub local_worktree: Option<String>,
    pub main_branch: String,
    pub latest_agent_run_id: Option<String>,
    pub latest_agent_run_outcome: Option<String>,
    pub pending_acceptance_criteria: i64,
    pub pending_validation_items: i64,
    pub failed_validation_items: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateRequirementStatus {
    pub status: String,
    pub waiver_reason: Option<String>,
    pub validated_base_commit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransitionTaskState {
    pub to_state: String,
    pub agent_run_id: Option<String>,
    #[serde(default)]
    pub repair_override: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordReviewDecision {
    pub decision: String,
    pub feedback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct AgentRun {
    pub id: String,
    pub task_id: String,
    pub task_queue_id: String,
    pub worker_actor_kind: String,
    pub worker_actor_id: String,
    pub worker_actor_display_name: String,
    pub worker_id: String,
    pub launcher_kind: String,
    pub lease_expires_at: String,
    pub last_heartbeat_at: Option<String>,
    pub outcome: Option<String>,
    pub failure_reason: Option<String>,
    pub failure_reason_code: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct LauncherSessionData {
    pub agent_run_id: String,
    pub launcher_kind: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub final_status: Option<String>,
    pub transcript_path: Option<String>,
    pub raw_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct AgentRunMetrics {
    pub agent_run_id: String,
    pub derivation_version: i64,
    pub duration_ms: Option<i64>,
    pub launcher_kind: String,
    pub final_status: Option<String>,
    pub exit_code: Option<i64>,
    pub timed_out: Option<i64>,
    pub unattended_question_detected: Option<i64>,
    pub blocking_ui_detected: Option<i64>,
    pub transcript_path: Option<String>,
    pub transcript_byte_size: Option<i64>,
    pub transcript_jsonl_event_count: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub tool_call_count: Option<i64>,
    pub tool_error_count: Option<i64>,
    pub repeated_failed_tool_attempt_count: Option<i64>,
    pub tool_call_counts_json: String,
    pub repeated_read_count: Option<i64>,
    pub repeated_tasker_context_fetch_count: Option<i64>,
    pub shell_command_counts_json: String,
    pub assistant_turn_count: Option<i64>,
    pub user_turn_count: Option<i64>,
    pub max_context_tokens: Option<i64>,
    pub efficiency_hints_json: String,
    pub warnings_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComputedAgentRunMetrics {
    pub agent_run_id: String,
    pub derivation_version: i64,
    pub duration_ms: Option<i64>,
    pub launcher_kind: String,
    pub final_status: Option<String>,
    pub exit_code: Option<i64>,
    pub timed_out: Option<i64>,
    pub unattended_question_detected: Option<i64>,
    pub blocking_ui_detected: Option<i64>,
    pub transcript_path: Option<String>,
    pub transcript_byte_size: Option<i64>,
    pub transcript_jsonl_event_count: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub tool_call_count: Option<i64>,
    pub tool_error_count: Option<i64>,
    pub repeated_failed_tool_attempt_count: Option<i64>,
    pub tool_call_counts_json: String,
    pub repeated_read_count: Option<i64>,
    pub repeated_tasker_context_fetch_count: Option<i64>,
    pub shell_command_counts_json: String,
    pub assistant_turn_count: Option<i64>,
    pub user_turn_count: Option<i64>,
    pub max_context_tokens: Option<i64>,
    pub efficiency_hints_json: String,
    pub warnings_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpsertLauncherSessionData {
    pub launcher_kind: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub final_status: Option<String>,
    pub transcript_path: Option<String>,
    pub raw_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunDetail {
    pub run: AgentRun,
    pub task: TaskDetail,
    pub launcher_session_data: Option<LauncherSessionData>,
    pub metrics: Option<AgentRunMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimedRun {
    pub run: AgentRun,
    pub task: TaskDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimNextInput {
    pub queue_key: String,
    pub worker_id: String,
    pub launcher_kind: String,
    pub lease_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinishRunInput {
    pub outcome: String,
    pub failure_reason: Option<String>,
    pub failure_reason_code: Option<String>,
    pub retry_hold_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorFailRunInput {
    pub failure_reason: String,
    pub failure_reason_code: Option<String>,
    pub retry_hold_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct IntegrationOutcome {
    pub id: String,
    pub task_id: String,
    pub agent_run_id: Option<String>,
    pub outcome_kind: String,
    pub reason_code: Option<String>,
    pub final_commit: Option<String>,
    pub pre_merge_head: Option<String>,
    pub message: Option<String>,
    pub retryable: bool,
    pub retry_attempt: Option<i64>,
    pub next_retry_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordIntegrationOutcomeInput {
    pub task_identifier: String,
    pub agent_run_id: Option<String>,
    pub outcome_kind: String,
    pub reason_code: String,
    pub final_commit: Option<String>,
    pub pre_merge_head: Option<String>,
    pub message: Option<String>,
    pub retryable: bool,
    pub retry_attempt: Option<i64>,
    pub retry_delay_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct IntegrationRetryStatus {
    pub queue_key: String,
    pub task_identifier: String,
    pub task_title: String,
    pub reason_code: String,
    pub retryable: bool,
    pub retry_attempt: Option<i64>,
    pub next_retry_at: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetryTaskInput {
    pub reason: String,
}

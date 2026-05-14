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

pub(crate) fn validate_transition(task: &Task, to_state: &str, actor: &Actor) -> Result<()> {
    match actor.kind.as_str() {
        "operator" | "review_agent" | "worker_agent" | "delegating_agent" => {}
        _ => anyhow::bail!(
            "State Transitions require an Operator, Review Agent, Worker Agent, or Delegating Agent actor"
        ),
    }
    if task.state == to_state {
        anyhow::bail!("Task is already in requested Task State");
    }
    let allowed = match task.state.as_str() {
        "backlog" => matches!(to_state, "ready" | "canceled"),
        "ready" => matches!(to_state, "in_progress" | "canceled"),
        "in_progress" => matches!(
            to_state,
            "human_review" | "integrating" | "done" | "canceled"
        ),
        "human_review" => matches!(to_state, "rework" | "integrating" | "canceled"),
        "rework" => matches!(
            to_state,
            "in_progress" | "human_review" | "integrating" | "canceled"
        ),
        "integrating" => matches!(to_state, "done" | "rework" | "canceled"),
        "done" | "canceled" => false,
        _ => false,
    };
    if !allowed {
        anyhow::bail!(
            "State Transition from {} to {to_state} is not allowed",
            task.state
        );
    }
    if task.review_required && to_state == "integrating" && task.state != "human_review" {
        anyhow::bail!(
            "Review-required Tasks must transition through Human Review before Integrating"
        );
    }
    if actor.kind == "worker_agent" {
        if to_state == "integrating" {
            if task.review_required {
                anyhow::bail!(
                    "Worker Agent cannot transition review-required Tasks to Integrating"
                );
            }
        } else if to_state != "human_review" && to_state != "canceled" {
            anyhow::bail!("Worker Agent cannot request this State Transition");
        }
    } else if actor.kind == "delegating_agent" && !(task.state == "backlog" && to_state == "ready")
    {
        anyhow::bail!(
            "Delegating Agent State Transitions are limited to Backlog to Ready refinement"
        );
    }
    Ok(())
}

pub(crate) fn requires_completion_gates(to_state: &str) -> bool {
    matches!(to_state, "human_review" | "integrating" | "done")
}

pub(crate) async fn ensure_ready_requirements_exist(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: &str,
) -> Result<()> {
    let criteria_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM acceptance_criteria WHERE task_id = ?")
            .bind(task_id)
            .fetch_one(&mut **tx)
            .await
            .context("failed to count Acceptance Criteria")?;
    let validation_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM validation_items WHERE task_id = ?")
            .bind(task_id)
            .fetch_one(&mut **tx)
            .await
            .context("failed to count Validation Items")?;
    if criteria_count == 0 || validation_count == 0 {
        anyhow::bail!(
            "Ready Tasks require at least one Acceptance Criterion and one Validation Item"
        );
    }
    Ok(())
}

pub(crate) async fn ensure_worker_owns_active_run(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: &str,
    agent_run_id: Option<&str>,
    actor: &Actor,
) -> Result<()> {
    let Some(agent_run_id) = agent_run_id else {
        anyhow::bail!("Worker Agent Integrating transition requires an active Agent Run ID");
    };
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM agent_runs
        WHERE id = ?
          AND task_id = ?
          AND outcome IS NULL
          AND lease_expires_at > CURRENT_TIMESTAMP
          AND worker_actor_kind = ?
          AND worker_actor_id = ?
        "#,
    )
    .bind(agent_run_id)
    .bind(task_id)
    .bind(&actor.kind)
    .bind(&actor.id)
    .fetch_one(&mut **tx)
    .await
    .context("failed to verify active Agent Run ownership")?;
    if count != 1 {
        anyhow::bail!("Worker Agent does not own an active Claim Lease for this Task");
    }
    Ok(())
}

pub(crate) async fn ensure_completion_gates_pass(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: &str,
) -> Result<()> {
    let unsatisfied_criteria: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM acceptance_criteria
        WHERE task_id = ? AND status NOT IN ('satisfied', 'waived')
        "#,
    )
    .bind(task_id)
    .fetch_one(&mut **tx)
    .await
    .context("failed to check Acceptance Criteria gates")?;
    let unpassed_validation: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM validation_items
        WHERE task_id = ? AND status NOT IN ('passed', 'waived')
        "#,
    )
    .bind(task_id)
    .fetch_one(&mut **tx)
    .await
    .context("failed to check Validation Item gates")?;
    if unsatisfied_criteria > 0 || unpassed_validation > 0 {
        anyhow::bail!(
            "State Transition requires all Acceptance Criteria and Validation Items to pass gates"
        );
    }
    Ok(())
}

pub(crate) fn validate_task_queue(input: &CreateTaskQueue) -> Result<()> {
    ensure_not_blank("Task Queue Key", &input.key)?;
    if input.key.contains('/') || input.key.contains('\\') {
        anyhow::bail!("Task Queue Key must not contain path separators");
    }
    ensure_not_blank("Task Queue name", &input.name)?;
    ensure_not_blank(
        "Managed Source Repository",
        &input.managed_source_repository,
    )?;
    ensure_not_blank("Main Branch", &input.main_branch)?;
    ensure_not_blank("Worktree Root", &input.worktree_root)?;
    ensure_not_blank("Branch Template", &input.branch_template)?;
    validate_queue_concurrency_limit(input.queue_concurrency_limit)?;
    Ok(())
}

pub(crate) fn validate_queue_concurrency_limit(limit: Option<i64>) -> Result<()> {
    if let Some(limit) = limit {
        if limit <= 0 {
            anyhow::bail!("Queue Concurrency Limit must be positive");
        }
    }
    Ok(())
}

pub(crate) fn validate_requirement_status(
    input: &UpdateRequirementStatus,
    actor: &Actor,
    allowed_statuses: &[&str],
) -> Result<()> {
    if !allowed_statuses.contains(&input.status.as_str()) {
        anyhow::bail!("invalid requirement status {}", input.status);
    }
    if input.status == "waived" {
        if actor.kind == "worker_agent" {
            anyhow::bail!("Worker Agents cannot create Waivers");
        }
        if actor.kind != "operator" && actor.kind != "review_agent" {
            anyhow::bail!("Waivers require an Operator or Review Agent actor");
        }
        match input.waiver_reason.as_deref() {
            Some(reason) if !reason.trim().is_empty() => {}
            _ => anyhow::bail!("Waivers require an explicit reason"),
        }
    }
    Ok(())
}

pub fn validate_create_task(input: &CreateTask) -> Result<()> {
    ensure_not_blank("title", &input.title)?;
    ensure_not_blank("Task Brief", &input.brief)?;
    validate_priority(&input.priority)?;
    validate_state(&input.state)?;
    if input.state != "backlog" && input.state != "ready" {
        anyhow::bail!("Bootstrap Task Creation only supports Backlog or Ready initial Task States");
    }
    if input.state == "ready"
        && (input.acceptance_criteria.is_empty() || input.validation_items.is_empty())
    {
        anyhow::bail!(
            "Ready Tasks require at least one Acceptance Criterion and one Validation Item"
        );
    }
    for criterion in &input.acceptance_criteria {
        ensure_not_blank("Acceptance Criterion", criterion)?;
    }
    for item in &input.validation_items {
        ensure_not_blank("Validation Item", item)?;
    }
    for hint in &input.conflict_hints {
        ensure_not_blank("Task Conflict Hint", hint)?;
    }
    for identifier in &input.blocking_task_identifiers {
        ensure_not_blank("Blocking Task Identifier", identifier)?;
    }
    Ok(())
}

pub(crate) fn validate_actor(actor: &Actor) -> Result<()> {
    ensure_not_blank("Actor kind", &actor.kind)?;
    ensure_not_blank("Actor id", &actor.id)?;
    ensure_not_blank("Actor display name", &actor.display_name)?;
    Ok(())
}

pub(crate) fn validate_child_task_actor(actor: &Actor) -> Result<()> {
    validate_actor(actor)?;
    if actor.kind == "operator" || actor.kind == "delegating_agent" || actor.kind == "worker_agent"
    {
        Ok(())
    } else {
        anyhow::bail!(
            "Child Task creation requires an Operator, Delegating Agent, or Worker Agent actor"
        )
    }
}

pub(crate) fn validate_refine_backlog_actor(actor: &Actor) -> Result<()> {
    validate_actor(actor)?;
    if actor.kind == "operator" || actor.kind == "delegating_agent" {
        Ok(())
    } else {
        anyhow::bail!("Backlog Task refinement requires an Operator or Delegating Agent actor")
    }
}

pub(crate) fn validate_worker_actor(actor: &Actor) -> Result<()> {
    validate_actor(actor)?;
    if actor.kind != "worker_agent" {
        anyhow::bail!("Agent Run mutations require a Worker Agent actor");
    }
    Ok(())
}

pub(crate) fn validate_operator_actor(actor: &Actor) -> Result<()> {
    validate_actor(actor)?;
    if actor.kind != "operator" {
        anyhow::bail!("recovery commands require an Operator actor");
    }
    Ok(())
}

pub(crate) fn validate_positive_seconds(field: &str, value: i64) -> Result<()> {
    if value <= 0 {
        anyhow::bail!("{field} must be positive");
    }
    Ok(())
}

pub(crate) fn validate_run_outcome(outcome: &str) -> Result<()> {
    match outcome {
        "completed" | "failed" | "canceled" => Ok(()),
        _ => anyhow::bail!("invalid Agent Run outcome {outcome}"),
    }
}

pub(crate) fn failure_reason_code_for_finish(input: &FinishRunInput) -> Result<Option<&str>> {
    let Some(code) = input.failure_reason_code.as_deref() else {
        return Ok((input.outcome == "failed").then_some("agent_run_failed"));
    };
    validate_failure_reason_code(code)?;
    if input.outcome == "completed" {
        anyhow::bail!("completed Agent Runs cannot have a failure reason code");
    }
    Ok(Some(code))
}

pub(crate) fn validate_failure_reason_code(code: &str) -> Result<()> {
    match code {
        "agent_run_failed"
        | "local_worktree_setup_failed"
        | "dirty_managed_source_repository"
        | "repo_operation_lock_held"
        | "migration_incompatible"
        | "stale_validation_base"
        | "launcher_start_failed"
        | "launcher_rpc_io_failed"
        | "launcher_exited"
        | "launcher_timeout"
        | "unattended_question"
        | "agent_gated_integration_failed"
        | "operator_failed"
        | "claim_lease_expired"
        | "task_canceled" => Ok(()),
        _ => anyhow::bail!("invalid Agent Run failure reason code {code}"),
    }
}

pub(crate) fn validate_priority(priority: &str) -> Result<()> {
    match priority {
        "urgent" | "high" | "normal" | "low" => Ok(()),
        _ => anyhow::bail!("invalid Priority {priority}"),
    }
}

pub(crate) fn validate_state(state: &str) -> Result<()> {
    match state {
        "backlog" | "ready" | "in_progress" | "human_review" | "rework" | "integrating"
        | "done" | "canceled" => Ok(()),
        _ => anyhow::bail!("invalid Task State {state}"),
    }
}

pub(crate) fn ensure_not_blank(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("{field} must not be blank");
    }
    Ok(())
}

pub(crate) fn normalized_tags(tags: &[String]) -> Vec<String> {
    let mut tags = tags
        .iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    tags.sort();
    tags.dedup();
    tags
}

pub(crate) fn normalized_conflict_hints(hints: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for hint in hints {
        let hint = hint.trim().to_string();
        if !hint.is_empty() && !normalized.contains(&hint) {
            normalized.push(hint);
        }
    }
    normalized
}

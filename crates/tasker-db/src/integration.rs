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

pub fn is_valid_integration_outcome_reason_code(reason_code: &str) -> bool {
    matches!(
        reason_code,
        "success"
            | "no_changes"
            | "uncommitted_local_worktree"
            | "stale_validated_base_commit"
            | "auto_refresh_success"
            | "auto_refresh_conflict"
            | "auto_refresh_validation_failed"
            | "auto_refresh_declined_missing_validation"
            | "task_branch_missing_main"
            | "dirty_managed_source_repository"
            | "repo_operation_lock_held"
            | "merge_conflict"
            | "cleanup_failure"
            | "unknown_operational_failure"
            | "unknown_work_change_failure"
            | "unknown_legacy"
    )
}

pub async fn record_integration_outcome(
    pool: &SqlitePool,
    input: &RecordIntegrationOutcomeInput,
    actor: &Actor,
) -> Result<IntegrationOutcome> {
    validate_actor(actor)?;
    if !matches!(
        input.outcome_kind.as_str(),
        "success" | "no_changes" | "work_change_failure" | "operational_failure"
    ) {
        anyhow::bail!("invalid Integration Outcome kind");
    }

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let task_id: String = sqlx::query_scalar("SELECT id FROM tasks WHERE identifier = ?")
        .bind(&input.task_identifier)
        .fetch_optional(&mut *tx)
        .await
        .with_context(|| format!("failed to load Task {}", input.task_identifier))?
        .with_context(|| format!("Task {} not found", input.task_identifier))?;

    if let Some(agent_run_id) = &input.agent_run_id {
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM agent_runs WHERE id = ?")
            .bind(agent_run_id)
            .fetch_optional(&mut *tx)
            .await
            .with_context(|| format!("failed to load Agent Run {agent_run_id}"))?;
        if exists.is_none() {
            anyhow::bail!("Agent Run {agent_run_id} not found");
        }
    }

    if !is_valid_integration_outcome_reason_code(&input.reason_code) {
        anyhow::bail!("invalid Integration Outcome reason code");
    }
    if input.retryable && input.outcome_kind != "operational_failure" {
        anyhow::bail!("only operational Integration Outcomes may be marked retryable");
    }
    if input.retry_attempt.is_some_and(|attempt| attempt <= 0) {
        anyhow::bail!("retry_attempt must be positive");
    }
    if input
        .retry_delay_seconds
        .is_some_and(|seconds| seconds <= 0)
    {
        anyhow::bail!("retry_delay_seconds must be positive");
    }

    let outcome_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO integration_outcomes (
            id, task_id, agent_run_id, outcome_kind, reason_code, final_commit, pre_merge_head, message,
            retryable, retry_attempt, next_retry_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CASE WHEN ? IS NULL THEN NULL ELSE datetime('now', '+' || ? || ' seconds') END)
        "#,
    )
    .bind(&outcome_id)
    .bind(&task_id)
    .bind(&input.agent_run_id)
    .bind(&input.outcome_kind)
    .bind(&input.reason_code)
    .bind(&input.final_commit)
    .bind(&input.pre_merge_head)
    .bind(&input.message)
    .bind(input.retryable)
    .bind(input.retry_attempt)
    .bind(input.retry_delay_seconds)
    .bind(input.retry_delay_seconds)
    .execute(&mut *tx)
    .await
    .context("failed to record Integration Outcome")?;

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "integration_outcome.recorded",
        "task",
        &task_id,
        serde_json::json!({
            "identifier": input.task_identifier,
            "agent_run_id": input.agent_run_id,
            "outcome_kind": input.outcome_kind,
            "reason_code": input.reason_code,
            "final_commit": input.final_commit,
            "pre_merge_head": input.pre_merge_head,
            "message": input.message,
            "retryable": input.retryable,
            "retry_attempt": input.retry_attempt,
            "retry_delay_seconds": input.retry_delay_seconds,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;

    sqlx::query_as::<_, IntegrationOutcome>(
        r#"
        SELECT id, task_id, agent_run_id, outcome_kind, reason_code, final_commit, pre_merge_head, message,
               retryable, retry_attempt, next_retry_at, created_at
        FROM integration_outcomes
        WHERE id = ?
        "#,
    )
    .bind(outcome_id)
    .fetch_one(pool)
    .await
    .context("failed to load recorded Integration Outcome")
}

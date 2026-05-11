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

pub async fn claim_next(
    pool: &SqlitePool,
    input: &ClaimNextInput,
    actor: &Actor,
) -> Result<Option<ClaimedRun>> {
    with_sqlite_write_retry(|| claim_next_once(pool, input, actor)).await
}

async fn claim_next_once(
    pool: &SqlitePool,
    input: &ClaimNextInput,
    actor: &Actor,
) -> Result<Option<ClaimedRun>> {
    validate_worker_actor(actor)?;
    ensure_not_blank("worker_id", &input.worker_id)?;
    ensure_not_blank("launcher_kind", &input.launcher_kind)?;
    validate_positive_seconds("lease_seconds", input.lease_seconds)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    expire_stale_agent_runs(&mut tx).await?;

    let queue = sqlx::query_as::<_, TaskQueue>(
        r#"
        SELECT id, key, name, delivery_backend, managed_source_repository, main_branch,
               worktree_root, branch_template, done_worktree_retention, queue_concurrency_limit, created_at, updated_at
        FROM task_queues
        WHERE key = ?
        "#,
    )
    .bind(&input.queue_key)
    .fetch_optional(&mut *tx)
    .await
    .with_context(|| format!("failed to load Task Queue {}", input.queue_key))?
    .with_context(|| format!("Task Queue {} not found", input.queue_key))?;

    if let Some(limit) = queue.queue_concurrency_limit {
        let active_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_runs WHERE task_queue_id = ? AND outcome IS NULL AND lease_expires_at > CURRENT_TIMESTAMP",
        )
        .bind(&queue.id)
        .fetch_one(&mut *tx)
        .await
        .context("failed to count active Agent Runs")?;
        if active_count >= limit {
            tx.commit().await.context("failed to commit transaction")?;
            return Ok(None);
        }
    }

    let claimed_task = sqlx::query_as::<_, Task>(
        r#"
        SELECT tasks.id, tasks.task_queue_id, task_queues.key AS task_queue_key, tasks.identifier,
               tasks.sequence, tasks.title, tasks.brief, tasks.priority, tasks.state,
               tasks.review_required, tasks.validated_base_commit, tasks.created_at, tasks.updated_at
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.task_queue_id = ?
          AND tasks.state IN ('ready', 'in_progress', 'rework')
          AND NOT EXISTS (
              SELECT 1 FROM agent_runs
              WHERE agent_runs.task_id = tasks.id AND agent_runs.outcome IS NULL
          )
          AND NOT EXISTS (
              SELECT 1 FROM task_retry_holds
              WHERE task_retry_holds.task_id = tasks.id AND task_retry_holds.hold_until > CURRENT_TIMESTAMP
          )
        ORDER BY
          CASE tasks.priority
            WHEN 'urgent' THEN 0
            WHEN 'high' THEN 1
            WHEN 'normal' THEN 2
            WHEN 'low' THEN 3
          END,
          tasks.created_at,
          tasks.identifier
        LIMIT 1
        "#,
    )
    .bind(&queue.id)
    .fetch_optional(&mut *tx)
    .await
    .context("failed to claim next Task")?;

    let Some(task) = claimed_task else {
        tx.commit().await.context("failed to commit transaction")?;
        return Ok(None);
    };

    if task.state == "ready" {
        sqlx::query(
            "UPDATE tasks SET state = 'in_progress', updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(&task.id)
        .execute(&mut *tx)
        .await
        .context("failed to move Task to In Progress")?;
        append_audit_event_in_tx(
            &mut tx,
            actor,
            "task.state_changed",
            "task",
            &task.id,
            serde_json::json!({
                "identifier": task.identifier,
                "from": "ready",
                "to": "in_progress",
            }),
        )
        .await?;
    }

    let run_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
                INSERT INTO agent_runs (
                    id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                    worker_actor_display_name, worker_id, launcher_kind, lease_expires_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now', '+' || ? || ' seconds'))
                "#,
    )
    .bind(&run_id)
    .bind(&task.id)
    .bind(&queue.id)
    .bind(&actor.kind)
    .bind(&actor.id)
    .bind(&actor.display_name)
    .bind(&input.worker_id)
    .bind(&input.launcher_kind)
    .bind(input.lease_seconds)
    .execute(&mut *tx)
    .await
    .context("failed to create Agent Run")?;
    let select_run_sql = agent_run_select_sql("WHERE id = ?");
    let run = sqlx::query_as::<_, AgentRun>(&select_run_sql)
        .bind(&run_id)
        .fetch_one(&mut *tx)
        .await
        .context("failed to load Agent Run")?;

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "agent_run.claimed",
        "agent_run",
        &run.id,
        serde_json::json!({
            "task_id": task.id,
            "task_identifier": task.identifier,
            "queue_key": queue.key,
            "lease_expires_at": run.lease_expires_at,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;
    let task = get_task_detail(pool, &task.identifier)
        .await?
        .with_context(|| format!("claimed Task {} was not found", task.identifier))?;

    Ok(Some(ClaimedRun { run, task }))
}

pub async fn heartbeat_run(
    pool: &SqlitePool,
    run_id: &str,
    lease_seconds: i64,
    actor: &Actor,
) -> Result<AgentRun> {
    validate_worker_actor(actor)?;
    validate_positive_seconds("lease_seconds", lease_seconds)?;
    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let run = sqlx::query_as::<_, AgentRun>(
        r#"
        UPDATE agent_runs
        SET last_heartbeat_at = CURRENT_TIMESTAMP,
            lease_expires_at = datetime('now', '+' || ? || ' seconds')
        WHERE id = ?
          AND outcome IS NULL
          AND lease_expires_at > CURRENT_TIMESTAMP
          AND worker_actor_kind = ?
          AND worker_actor_id = ?
        RETURNING id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                  worker_actor_display_name, worker_id, launcher_kind, lease_expires_at,
                  last_heartbeat_at, outcome, failure_reason, failure_reason_code, created_at, finished_at
        "#,
    )
    .bind(lease_seconds)
    .bind(run_id)
    .bind(&actor.kind)
    .bind(&actor.id)
    .fetch_optional(&mut *tx)
    .await
    .context("failed to heartbeat Agent Run")?
    .with_context(|| format!("active Agent Run {run_id} not found for actor"))?;

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "agent_run.heartbeat",
        "agent_run",
        &run.id,
        serde_json::json!({ "lease_expires_at": run.lease_expires_at }),
    )
    .await?;
    tx.commit().await.context("failed to commit transaction")?;
    Ok(run)
}

pub async fn finish_run(
    pool: &SqlitePool,
    run_id: &str,
    input: &FinishRunInput,
    actor: &Actor,
) -> Result<AgentRun> {
    validate_worker_actor(actor)?;
    validate_run_outcome(&input.outcome)?;
    if let Some(seconds) = input.retry_hold_seconds {
        validate_positive_seconds("retry_hold_seconds", seconds)?;
    }
    let failure_reason_code = failure_reason_code_for_finish(input)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let run = sqlx::query_as::<_, AgentRun>(
        r#"
        UPDATE agent_runs
        SET outcome = ?, failure_reason = ?, failure_reason_code = ?, finished_at = CURRENT_TIMESTAMP
        WHERE id = ?
          AND outcome IS NULL
          AND lease_expires_at > CURRENT_TIMESTAMP
          AND worker_actor_kind = ?
          AND worker_actor_id = ?
        RETURNING id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                  worker_actor_display_name, worker_id, launcher_kind, lease_expires_at,
                  last_heartbeat_at, outcome, failure_reason, failure_reason_code, created_at, finished_at
        "#,
    )
    .bind(&input.outcome)
    .bind(&input.failure_reason)
    .bind(failure_reason_code)
    .bind(run_id)
    .bind(&actor.kind)
    .bind(&actor.id)
    .fetch_optional(&mut *tx)
    .await
    .context("failed to finish Agent Run")?
    .with_context(|| format!("active Agent Run {run_id} not found for actor"))?;

    if input.outcome == "failed" {
        let seconds = input.retry_hold_seconds.unwrap_or(60);
        let reason = input
            .failure_reason
            .clone()
            .unwrap_or_else(|| "Agent Run failed".to_string());
        sqlx::query(
            r#"
            INSERT INTO task_retry_holds (task_id, agent_run_id, hold_until, reason)
            VALUES (?, ?, datetime('now', '+' || ? || ' seconds'), ?)
            ON CONFLICT(task_id) DO UPDATE SET
                agent_run_id = excluded.agent_run_id,
                hold_until = excluded.hold_until,
                reason = excluded.reason,
                created_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&run.task_id)
        .bind(&run.id)
        .bind(seconds)
        .bind(&reason)
        .execute(&mut *tx)
        .await
        .context("failed to create Retry Hold")?;
        append_audit_event_in_tx(
            &mut tx,
            actor,
            "task.retry_hold_created",
            "task",
            &run.task_id,
            serde_json::json!({
                "agent_run_id": run.id,
                "hold_seconds": seconds,
                "reason": reason,
                "failure_reason_code": run.failure_reason_code,
            }),
        )
        .await?;
    } else if input.outcome == "completed" {
        let deleted = sqlx::query("DELETE FROM task_retry_holds WHERE task_id = ?")
            .bind(&run.task_id)
            .execute(&mut *tx)
            .await
            .context("failed to clear Retry Hold")?;
        if deleted.rows_affected() > 0 {
            append_audit_event_in_tx(
                &mut tx,
                actor,
                "task.retry_hold_cleared",
                "task",
                &run.task_id,
                serde_json::json!({ "agent_run_id": run.id }),
            )
            .await?;
        }
    }

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "agent_run.finished",
        "agent_run",
        &run.id,
        serde_json::json!({
            "outcome": input.outcome,
            "failure_reason": input.failure_reason,
            "failure_reason_code": run.failure_reason_code,
            "retry_hold_seconds": input.retry_hold_seconds,
        }),
    )
    .await?;
    tx.commit().await.context("failed to commit transaction")?;
    refresh_agent_run_metrics(pool, &run.id).await?;
    Ok(run)
}

pub async fn operator_fail_run(
    pool: &SqlitePool,
    run_id: &str,
    input: &OperatorFailRunInput,
    actor: &Actor,
) -> Result<AgentRun> {
    validate_operator_actor(actor)?;
    ensure_not_blank("failure reason", &input.failure_reason)?;
    if let Some(seconds) = input.retry_hold_seconds {
        validate_positive_seconds("retry_hold_seconds", seconds)?;
    }
    let failure_reason_code = input
        .failure_reason_code
        .as_deref()
        .unwrap_or("operator_failed");
    validate_failure_reason_code(failure_reason_code)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let run = sqlx::query_as::<_, AgentRun>(
        r#"
        UPDATE agent_runs
        SET outcome = 'failed', failure_reason = ?, failure_reason_code = ?, finished_at = CURRENT_TIMESTAMP
        WHERE id = ? AND outcome IS NULL
        RETURNING id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                  worker_actor_display_name, worker_id, launcher_kind, lease_expires_at,
                  last_heartbeat_at, outcome, failure_reason, failure_reason_code, created_at, finished_at
        "#,
    )
    .bind(input.failure_reason.trim())
    .bind(failure_reason_code)
    .bind(run_id)
    .fetch_optional(&mut *tx)
    .await
    .context("failed to fail Agent Run")?
    .with_context(|| format!("active Agent Run {run_id} not found"))?;

    let seconds = input.retry_hold_seconds.unwrap_or(60);
    sqlx::query(
        r#"
        INSERT INTO task_retry_holds (task_id, agent_run_id, hold_until, reason)
        VALUES (?, ?, datetime('now', '+' || ? || ' seconds'), ?)
        ON CONFLICT(task_id) DO UPDATE SET
            agent_run_id = excluded.agent_run_id,
            hold_until = excluded.hold_until,
            reason = excluded.reason,
            created_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(&run.task_id)
    .bind(&run.id)
    .bind(seconds)
    .bind(input.failure_reason.trim())
    .execute(&mut *tx)
    .await
    .context("failed to create Retry Hold")?;

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task.retry_hold_created",
        "task",
        &run.task_id,
        serde_json::json!({
            "agent_run_id": run.id,
            "hold_seconds": seconds,
            "reason": input.failure_reason.trim(),
            "failure_reason_code": run.failure_reason_code,
        }),
    )
    .await?;
    append_audit_event_in_tx(
        &mut tx,
        actor,
        "agent_run.operator_failed",
        "agent_run",
        &run.id,
        serde_json::json!({
            "reason": input.failure_reason.trim(),
            "failure_reason_code": run.failure_reason_code,
            "retry_hold_seconds": seconds,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;
    refresh_agent_run_metrics(pool, &run.id).await?;
    Ok(run)
}

pub async fn retry_task(
    pool: &SqlitePool,
    identifier: &str,
    input: &RetryTaskInput,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_operator_actor(actor)?;
    ensure_not_blank("retry reason", &input.reason)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    expire_stale_agent_runs(&mut tx).await?;

    let task = sqlx::query_as::<_, Task>(
        r#"
        SELECT tasks.id, tasks.task_queue_id, task_queues.key AS task_queue_key,
               tasks.identifier, tasks.sequence, tasks.title, tasks.brief, tasks.priority,
               tasks.state, tasks.review_required, tasks.validated_base_commit, tasks.created_at, tasks.updated_at
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.identifier = ?
        "#,
    )
    .bind(identifier)
    .fetch_optional(&mut *tx)
    .await
    .with_context(|| format!("failed to load Task {identifier}"))?
    .with_context(|| format!("Task {identifier} not found"))?;

    if !matches!(
        task.state.as_str(),
        "in_progress" | "rework" | "integrating" | "canceled"
    ) {
        anyhow::bail!(
            "Retry recovery requires Task State in_progress, rework, integrating, or canceled; current state is {}",
            task.state
        );
    }
    ensure_ready_requirements_exist(&mut tx, &task.id).await?;

    let active_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM agent_runs
        WHERE task_id = ? AND outcome IS NULL AND lease_expires_at > CURRENT_TIMESTAMP
        "#,
    )
    .bind(&task.id)
    .fetch_one(&mut *tx)
    .await
    .context("failed to count active Agent Runs")?;
    if active_count > 0 {
        anyhow::bail!("cannot retry Task while it has active Agent Runs");
    }

    let previous_run_outcome: Option<String> = sqlx::query_scalar(
        r#"
        SELECT outcome FROM agent_runs
        WHERE task_id = ?
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(&task.id)
    .fetch_optional(&mut *tx)
    .await
    .context("failed to load latest Agent Run outcome")?;

    let deleted_holds = sqlx::query("DELETE FROM task_retry_holds WHERE task_id = ?")
        .bind(&task.id)
        .execute(&mut *tx)
        .await
        .context("failed to clear Retry Hold")?;
    if deleted_holds.rows_affected() > 0 {
        append_audit_event_in_tx(
            &mut tx,
            actor,
            "task.retry_hold_cleared",
            "task",
            &task.id,
            serde_json::json!({ "identifier": identifier, "reason": "operator retry" }),
        )
        .await?;
    }

    let update = sqlx::query(
        "UPDATE tasks SET state = 'ready', updated_at = CURRENT_TIMESTAMP WHERE id = ? AND state = ?",
    )
    .bind(&task.id)
    .bind(&task.state)
    .execute(&mut *tx)
    .await
    .context("failed to move Task to Ready for retry")?;
    if update.rows_affected() != 1 {
        anyhow::bail!("Task State changed while attempting retry recovery");
    }

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task.retry_requested",
        "task",
        &task.id,
        serde_json::json!({
            "identifier": identifier,
            "from": task.state,
            "to": "ready",
            "reason": input.reason.trim(),
            "latest_agent_run_outcome": previous_run_outcome,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;
    get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("retried Task {identifier} was not found"))
}

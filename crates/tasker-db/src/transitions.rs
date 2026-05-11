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

pub async fn transition_task_state(
    pool: &SqlitePool,
    identifier: &str,
    input: &TransitionTaskState,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_actor(actor)?;
    validate_state(&input.to_state)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let task = sqlx::query_as::<_, Task>(
        r#"
        SELECT
            tasks.id,
            tasks.task_queue_id,
            task_queues.key AS task_queue_key,
            tasks.identifier,
            tasks.sequence,
            tasks.title,
            tasks.brief,
            tasks.priority,
            tasks.state,
            tasks.review_required,
            tasks.validated_base_commit,
            tasks.created_at,
            tasks.updated_at
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

    if input.repair_override && actor.kind != "operator" {
        anyhow::bail!("Repair Override requires an Operator actor");
    }
    validate_transition(&task, &input.to_state, actor)?;
    if input.to_state == "ready" {
        ensure_ready_requirements_exist(&mut tx, &task.id).await?;
    }
    if requires_completion_gates(&input.to_state) && !input.repair_override {
        ensure_completion_gates_pass(&mut tx, &task.id).await?;
        let unresolved = unresolved_blocking_task_count(&mut tx, &task.id).await?;
        if unresolved > 0 {
            anyhow::bail!("Blocked Tasks cannot transition to Human Review, Integrating, or Done until all Blocking Tasks are Done");
        }
    }
    if actor.kind == "worker_agent" {
        ensure_worker_owns_active_run(&mut tx, &task.id, input.agent_run_id.as_deref(), actor)
            .await?;
    }

    let update = sqlx::query(
        "UPDATE tasks SET state = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ? AND state = ?",
    )
    .bind(&input.to_state)
    .bind(&task.id)
    .bind(&task.state)
    .execute(&mut *tx)
    .await
    .context("failed to transition Task State")?;
    if update.rows_affected() != 1 {
        anyhow::bail!("Task State changed while attempting State Transition");
    }
    let deleted_holds = sqlx::query("DELETE FROM task_retry_holds WHERE task_id = ?")
        .bind(&task.id)
        .execute(&mut *tx)
        .await
        .context("failed to clear Retry Hold after State Transition")?;
    if deleted_holds.rows_affected() > 0 {
        append_audit_event_in_tx(
            &mut tx,
            actor,
            "task.retry_hold_cleared",
            "task",
            &task.id,
            serde_json::json!({ "identifier": identifier, "reason": "Task State changed" }),
        )
        .await?;
    }
    if input.to_state == "canceled" {
        let canceled_runs = sqlx::query(
            r#"
            UPDATE agent_runs
            SET outcome = 'canceled', finished_at = CURRENT_TIMESTAMP, failure_reason = 'Task canceled', failure_reason_code = 'task_canceled'
            WHERE task_id = ? AND outcome IS NULL
            "#,
        )
        .bind(&task.id)
        .execute(&mut *tx)
        .await
        .context("failed to cancel active Agent Runs")?;
        if canceled_runs.rows_affected() > 0 {
            append_audit_event_in_tx(
                &mut tx,
                actor,
                "agent_run.canceled_for_task",
                "task",
                &task.id,
                serde_json::json!({
                    "identifier": identifier,
                    "canceled_runs": canceled_runs.rows_affected(),
                }),
            )
            .await?;
        }
    }
    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task.state_transitioned",
        "task",
        &task.id,
        serde_json::json!({
            "identifier": identifier,
            "from": task.state,
            "to": input.to_state,
            "repair_override": input.repair_override,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;
    get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("transitioned Task {identifier} was not found"))
}

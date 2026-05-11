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

pub async fn create_task_queue(
    pool: &SqlitePool,
    input: &CreateTaskQueue,
    actor: &Actor,
) -> Result<TaskQueue> {
    with_sqlite_write_retry(|| create_task_queue_once(pool, input, actor)).await
}

async fn create_task_queue_once(
    pool: &SqlitePool,
    input: &CreateTaskQueue,
    actor: &Actor,
) -> Result<TaskQueue> {
    validate_actor(actor)?;
    validate_task_queue(input)?;
    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let queue_id = Uuid::new_v4().to_string();
    let audit_id = Uuid::new_v4().to_string();
    let payload_json = serde_json::to_string(input).context("failed to encode audit payload")?;

    sqlx::query(
        r#"
        INSERT INTO task_queues (
            id,
            key,
            name,
            delivery_backend,
            managed_source_repository,
            main_branch,
            worktree_root,
            branch_template,
            done_worktree_retention,
            queue_concurrency_limit
        ) VALUES (?, ?, ?, 'local_worktree', ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&queue_id)
    .bind(&input.key)
    .bind(&input.name)
    .bind(&input.managed_source_repository)
    .bind(&input.main_branch)
    .bind(&input.worktree_root)
    .bind(&input.branch_template)
    .bind(input.done_worktree_retention)
    .bind(input.queue_concurrency_limit)
    .execute(&mut *tx)
    .await
    .with_context(|| format!("failed to create Task Queue {}", input.key))?;

    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id,
            actor_kind,
            actor_id,
            actor_display_name,
            event_type,
            subject_type,
            subject_id,
            payload_json
        ) VALUES (?, ?, ?, ?, 'task_queue.created', 'task_queue', ?, ?)
        "#,
    )
    .bind(&audit_id)
    .bind(&actor.kind)
    .bind(&actor.id)
    .bind(&actor.display_name)
    .bind(&queue_id)
    .bind(payload_json)
    .execute(&mut *tx)
    .await
    .context("failed to append audit event")?;

    tx.commit().await.context("failed to commit transaction")?;

    get_task_queue(pool, &input.key)
        .await?
        .with_context(|| format!("created Task Queue {} was not found", input.key))
}

pub async fn get_task_queue(pool: &SqlitePool, key: &str) -> Result<Option<TaskQueue>> {
    sqlx::query_as::<_, TaskQueue>(
        r#"
        SELECT
            id,
            key,
            name,
            delivery_backend,
            managed_source_repository,
            main_branch,
            worktree_root,
            branch_template,
            done_worktree_retention,
            queue_concurrency_limit,
            created_at,
            updated_at
        FROM task_queues
        WHERE key = ?
        "#,
    )
    .bind(key)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to load Task Queue {key}"))
}

pub async fn list_task_queues(pool: &SqlitePool) -> Result<Vec<TaskQueue>> {
    sqlx::query_as::<_, TaskQueue>(
        r#"
        SELECT
            id,
            key,
            name,
            delivery_backend,
            managed_source_repository,
            main_branch,
            worktree_root,
            branch_template,
            done_worktree_retention,
            queue_concurrency_limit,
            created_at,
            updated_at
        FROM task_queues
        ORDER BY key
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to list Task Queues")
}

pub async fn update_task_queue_concurrency_limit(
    pool: &SqlitePool,
    key: &str,
    input: &UpdateQueueConcurrencyLimit,
    actor: &Actor,
) -> Result<TaskQueue> {
    validate_actor(actor)?;
    validate_queue_concurrency_limit(input.queue_concurrency_limit)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let existing = sqlx::query_as::<_, TaskQueue>(
        r#"
        SELECT
            id, key, name, delivery_backend, managed_source_repository, main_branch,
            worktree_root, branch_template, done_worktree_retention, queue_concurrency_limit,
            created_at, updated_at
        FROM task_queues
        WHERE key = ?
        "#,
    )
    .bind(key)
    .fetch_optional(&mut *tx)
    .await
    .with_context(|| format!("failed to load Task Queue {key}"))?
    .with_context(|| format!("Task Queue {key} not found"))?;

    sqlx::query(
        r#"
        UPDATE task_queues
        SET queue_concurrency_limit = ?,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(input.queue_concurrency_limit)
    .bind(&existing.id)
    .execute(&mut *tx)
    .await
    .with_context(|| format!("failed to update Queue Concurrency Limit for Task Queue {key}"))?;

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task_queue.concurrency_limit_updated",
        "task_queue",
        &existing.id,
        serde_json::json!({
            "key": key,
            "previous_queue_concurrency_limit": existing.queue_concurrency_limit,
            "queue_concurrency_limit": input.queue_concurrency_limit,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;

    get_task_queue(pool, key)
        .await?
        .with_context(|| format!("updated Task Queue {key} was not found"))
}

pub async fn list_audit_events(pool: &SqlitePool) -> Result<Vec<AuditEvent>> {
    sqlx::query_as::<_, AuditEvent>(
        r#"
        SELECT
            id,
            actor_kind,
            actor_id,
            actor_display_name,
            event_type,
            subject_type,
            subject_id,
            payload_json,
            created_at
        FROM audit_events
        ORDER BY created_at, id
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to list Audit Events")
}

pub async fn list_task_queue_audit_events(pool: &SqlitePool, key: &str) -> Result<Vec<AuditEvent>> {
    sqlx::query_as::<_, AuditEvent>(
        r#"
        SELECT
            audit_events.id,
            audit_events.actor_kind,
            audit_events.actor_id,
            audit_events.actor_display_name,
            audit_events.event_type,
            audit_events.subject_type,
            audit_events.subject_id,
            audit_events.payload_json,
            audit_events.created_at
        FROM audit_events
        JOIN task_queues ON task_queues.id = audit_events.subject_id
        WHERE audit_events.subject_type = 'task_queue'
          AND task_queues.key = ?
        ORDER BY audit_events.created_at, audit_events.id
        "#,
    )
    .bind(key)
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to list Audit Events for Task Queue {key}"))
}

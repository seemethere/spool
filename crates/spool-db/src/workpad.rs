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

pub async fn update_workpad_note(
    pool: &SqlitePool,
    identifier: &str,
    body: &str,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_actor(actor)?;
    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let task_id: String = sqlx::query_scalar("SELECT id FROM tasks WHERE identifier = ?")
        .bind(identifier)
        .fetch_optional(&mut *tx)
        .await
        .with_context(|| format!("failed to load Task {identifier}"))?
        .with_context(|| format!("Task {identifier} not found"))?;

    let existing = sqlx::query_as::<_, WorkpadNote>(
        "SELECT id, task_id, body, created_at, updated_at FROM workpad_notes WHERE task_id = ?",
    )
    .bind(&task_id)
    .fetch_optional(&mut *tx)
    .await
    .context("failed to load Workpad Note")?;

    let workpad_note_id = if let Some(note) = existing {
        sqlx::query("INSERT INTO workpad_revisions (id, workpad_note_id, body) VALUES (?, ?, ?)")
            .bind(Uuid::new_v4().to_string())
            .bind(&note.id)
            .bind(&note.body)
            .execute(&mut *tx)
            .await
            .context("failed to create Workpad Revision")?;
        sqlx::query(
            "UPDATE workpad_notes SET body = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(body)
        .bind(&note.id)
        .execute(&mut *tx)
        .await
        .context("failed to update Workpad Note")?;
        note.id
    } else {
        let note_id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO workpad_notes (id, task_id, body) VALUES (?, ?, ?)")
            .bind(&note_id)
            .bind(&task_id)
            .bind(body)
            .execute(&mut *tx)
            .await
            .context("failed to create Workpad Note")?;
        note_id
    };

    let payload_json = serde_json::json!({
        "identifier": identifier,
        "workpad_note_id": workpad_note_id,
    })
    .to_string();
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_kind, actor_id, actor_display_name, event_type, subject_type, subject_id, payload_json
        ) VALUES (?, ?, ?, ?, 'workpad_note.updated', 'task', ?, ?)
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&actor.kind)
    .bind(&actor.id)
    .bind(&actor.display_name)
    .bind(&task_id)
    .bind(payload_json)
    .execute(&mut *tx)
    .await
    .context("failed to append audit event")?;

    tx.commit().await.context("failed to commit transaction")?;

    get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("updated Task {identifier} was not found"))
}

pub async fn count_workpad_revisions(pool: &SqlitePool, identifier: &str) -> Result<i64> {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM workpad_revisions
        JOIN workpad_notes ON workpad_notes.id = workpad_revisions.workpad_note_id
        JOIN tasks ON tasks.id = workpad_notes.task_id
        WHERE tasks.identifier = ?
        "#,
    )
    .bind(identifier)
    .fetch_one(pool)
    .await
    .context("failed to count Workpad Revisions")
}

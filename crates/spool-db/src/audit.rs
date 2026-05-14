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

pub async fn list_task_audit_events(
    pool: &SqlitePool,
    identifier: &str,
) -> Result<Vec<AuditEvent>> {
    let task_id: String = sqlx::query_scalar("SELECT id FROM tasks WHERE identifier = ?")
        .bind(identifier)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("failed to load Task {identifier}"))?
        .with_context(|| format!("Task {identifier} not found"))?;

    sqlx::query_as::<_, AuditEvent>(
        r#"
        SELECT id, actor_kind, actor_id, actor_display_name, event_type, subject_type, subject_id, payload_json, created_at
        FROM audit_events
        WHERE subject_type = 'task' AND subject_id = ?
        ORDER BY created_at, id
        "#,
    )
    .bind(task_id)
    .fetch_all(pool)
    .await
    .context("failed to list task Audit Events")
}

pub(crate) async fn append_audit_event_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    actor: &Actor,
    event_type: &str,
    subject_type: &str,
    subject_id: &str,
    payload: serde_json::Value,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_kind, actor_id, actor_display_name, event_type, subject_type, subject_id, payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&actor.kind)
    .bind(&actor.id)
    .bind(&actor.display_name)
    .bind(event_type)
    .bind(subject_type)
    .bind(subject_id)
    .bind(payload.to_string())
    .execute(&mut **tx)
    .await
    .context("failed to append audit event")?;
    Ok(())
}

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

pub async fn update_acceptance_criterion_status(
    pool: &SqlitePool,
    identifier: &str,
    position: i64,
    input: &UpdateRequirementStatus,
    actor: &Actor,
) -> Result<TaskDetail> {
    update_requirement_status(
        pool,
        identifier,
        position,
        input,
        actor,
        RequirementKind {
            table: "acceptance_criteria",
            event_type: "acceptance_criterion.status_updated",
            allowed_statuses: &["pending", "satisfied", "waived"],
        },
    )
    .await
}

pub async fn update_validation_item_status(
    pool: &SqlitePool,
    identifier: &str,
    position: i64,
    input: &UpdateRequirementStatus,
    actor: &Actor,
) -> Result<TaskDetail> {
    update_requirement_status(
        pool,
        identifier,
        position,
        input,
        actor,
        RequirementKind {
            table: "validation_items",
            event_type: "validation_item.status_updated",
            allowed_statuses: &["pending", "passed", "failed", "waived"],
        },
    )
    .await
}

struct RequirementKind {
    table: &'static str,
    event_type: &'static str,
    allowed_statuses: &'static [&'static str],
}

async fn update_requirement_status(
    pool: &SqlitePool,
    identifier: &str,
    position: i64,
    input: &UpdateRequirementStatus,
    actor: &Actor,
    kind: RequirementKind,
) -> Result<TaskDetail> {
    validate_actor(actor)?;
    validate_requirement_status(input, actor, kind.allowed_statuses)?;
    if position < 1 {
        anyhow::bail!("requirement position must be at least 1");
    }

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let task_id: String = sqlx::query_scalar("SELECT id FROM tasks WHERE identifier = ?")
        .bind(identifier)
        .fetch_optional(&mut *tx)
        .await
        .with_context(|| format!("failed to load Task {identifier}"))?
        .with_context(|| format!("Task {identifier} not found"))?;

    let select_sql = format!(
        "SELECT status FROM {} WHERE task_id = ? AND position = ?",
        kind.table
    );
    let previous_status: String = sqlx::query_scalar(&select_sql)
        .bind(&task_id)
        .bind(position)
        .fetch_optional(&mut *tx)
        .await
        .context("failed to load requirement status")?
        .with_context(|| format!("requirement at position {position} not found"))?;

    let waiver_reason = if input.status == "waived" {
        input
            .waiver_reason
            .as_ref()
            .map(|reason| reason.trim().to_string())
    } else {
        None
    };
    let update_sql = format!(
        "UPDATE {} SET status = ?, waiver_reason = ?, updated_at = CURRENT_TIMESTAMP WHERE task_id = ? AND position = ?",
        kind.table
    );
    sqlx::query(&update_sql)
        .bind(&input.status)
        .bind(&waiver_reason)
        .bind(&task_id)
        .bind(position)
        .execute(&mut *tx)
        .await
        .context("failed to update requirement status")?;

    if kind.table == "validation_items" {
        let validated_base_commit = if input.status == "passed" {
            input
                .validated_base_commit
                .as_deref()
                .map(str::trim)
                .filter(|commit| !commit.is_empty())
        } else {
            None
        };
        sqlx::query("UPDATE tasks SET validated_base_commit = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(validated_base_commit)
            .bind(&task_id)
            .execute(&mut *tx)
            .await
            .context("failed to update Validated Base Commit")?;
    }

    let payload_json = serde_json::json!({
        "identifier": identifier,
        "position": position,
        "previous_status": previous_status,
        "status": input.status,
        "waiver_reason": waiver_reason,
        "validated_base_commit": input.validated_base_commit,
    })
    .to_string();
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_kind, actor_id, actor_display_name, event_type, subject_type, subject_id, payload_json
        ) VALUES (?, ?, ?, ?, ?, 'task', ?, ?)
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&actor.kind)
    .bind(&actor.id)
    .bind(&actor.display_name)
    .bind(kind.event_type)
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

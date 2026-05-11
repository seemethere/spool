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

pub async fn record_review_decision(
    pool: &SqlitePool,
    identifier: &str,
    input: &RecordReviewDecision,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_review_decision_actor(actor)?;
    let decision = normalize_review_decision(&input.decision)?;
    let to_state = match decision.as_str() {
        "approve" => "integrating",
        "rework" => "rework",
        _ => unreachable!("validated Review Decision"),
    };

    let feedback = input
        .feedback
        .as_deref()
        .map(str::trim)
        .filter(|feedback| !feedback.is_empty());
    if decision == "rework" && feedback.is_none() {
        anyhow::bail!("Rework Review Decisions require human feedback");
    }

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let task = load_task_for_review_decision(&mut tx, identifier).await?;
    if task.state != "human_review" {
        anyhow::bail!("Review Decisions can only be recorded for Tasks in Human Review");
    }

    validate_transition(&task, to_state, actor)?;
    if requires_completion_gates(to_state) {
        ensure_completion_gates_pass(&mut tx, &task.id).await?;
        let unresolved = unresolved_blocking_task_count(&mut tx, &task.id).await?;
        if unresolved > 0 {
            anyhow::bail!("Blocked Tasks cannot transition to Human Review, Integrating, or Done until all Blocking Tasks are Done");
        }
    }

    let feedback_captured = if let Some(feedback) = feedback {
        append_review_feedback_to_workpad(&mut tx, &task, &decision, feedback, actor).await?;
        true
    } else {
        false
    };

    let update = sqlx::query(
        "UPDATE tasks SET state = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ? AND state = ?",
    )
    .bind(to_state)
    .bind(&task.id)
    .bind(&task.state)
    .execute(&mut *tx)
    .await
    .context("failed to transition Task State for Review Decision")?;
    if update.rows_affected() != 1 {
        anyhow::bail!("Task State changed while recording Review Decision");
    }

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task.review_decision_recorded",
        "task",
        &task.id,
        serde_json::json!({
            "identifier": identifier,
            "decision": decision,
            "to_state": to_state,
            "feedback_captured": feedback_captured,
        }),
    )
    .await?;
    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task.state_transitioned",
        "task",
        &task.id,
        serde_json::json!({
            "identifier": identifier,
            "from": task.state,
            "to": to_state,
            "repair_override": false,
            "review_decision": decision,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;
    get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("reviewed Task {identifier} was not found"))
}

fn normalize_review_decision(decision: &str) -> Result<String> {
    let normalized = decision
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-'], "_");
    match normalized.as_str() {
        "approve" | "approved" => Ok("approve".to_string()),
        "rework" | "request_rework" | "changes_requested" => Ok("rework".to_string()),
        _ => anyhow::bail!("invalid Review Decision {decision}"),
    }
}

fn validate_review_decision_actor(actor: &Actor) -> Result<()> {
    validate_actor(actor)?;
    if actor.kind == "operator" || actor.kind == "review_agent" {
        Ok(())
    } else {
        anyhow::bail!("Review Decisions require an Operator or Review Agent actor")
    }
}

async fn load_task_for_review_decision(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    identifier: &str,
) -> Result<Task> {
    sqlx::query_as::<_, Task>(
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
    .fetch_optional(&mut **tx)
    .await
    .with_context(|| format!("failed to load Task {identifier}"))?
    .with_context(|| format!("Task {identifier} not found"))
}

async fn append_review_feedback_to_workpad(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task: &Task,
    decision: &str,
    feedback: &str,
    actor: &Actor,
) -> Result<()> {
    let existing = sqlx::query_as::<_, WorkpadNote>(
        "SELECT id, task_id, body, created_at, updated_at FROM workpad_notes WHERE task_id = ?",
    )
    .bind(&task.id)
    .fetch_optional(&mut **tx)
    .await
    .context("failed to load Workpad Note")?;

    let decision_label = match decision {
        "approve" => "Approve",
        "rework" => "Rework",
        _ => unreachable!("validated Review Decision"),
    };
    let entry = format!("## Review Decision: {decision_label}\n\nHuman feedback:\n\n{feedback}");
    let workpad_note_id = if let Some(note) = existing {
        sqlx::query("INSERT INTO workpad_revisions (id, workpad_note_id, body) VALUES (?, ?, ?)")
            .bind(Uuid::new_v4().to_string())
            .bind(&note.id)
            .bind(&note.body)
            .execute(&mut **tx)
            .await
            .context("failed to create Workpad Revision")?;
        let body = if note.body.trim().is_empty() {
            entry
        } else {
            format!("{}\n\n{}", note.body.trim_end(), entry)
        };
        sqlx::query(
            "UPDATE workpad_notes SET body = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(&body)
        .bind(&note.id)
        .execute(&mut **tx)
        .await
        .context("failed to update Workpad Note")?;
        note.id
    } else {
        let workpad_note_id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO workpad_notes (id, task_id, body) VALUES (?, ?, ?)")
            .bind(&workpad_note_id)
            .bind(&task.id)
            .bind(&entry)
            .execute(&mut **tx)
            .await
            .context("failed to create Workpad Note")?;
        workpad_note_id
    };

    append_audit_event_in_tx(
        tx,
        actor,
        "workpad_note.updated",
        "task",
        &task.id,
        serde_json::json!({
            "identifier": task.identifier,
            "workpad_note_id": workpad_note_id,
            "reason": "Review Decision feedback",
        }),
    )
    .await
}

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

pub async fn create_task(
    pool: &SqlitePool,
    input: &CreateTask,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_actor(actor)?;
    validate_create_task(input)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
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

    let sequence: i64 = sqlx::query_scalar(
        r#"
        UPDATE task_queues
        SET next_task_sequence = next_task_sequence + 1,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        RETURNING next_task_sequence - 1
        "#,
    )
    .bind(&queue.id)
    .fetch_one(&mut *tx)
    .await
    .context("failed to allocate Task Identifier sequence")?;

    let task_id = Uuid::new_v4().to_string();
    let identifier = format!("{}-{}", queue.key, sequence);
    sqlx::query(
        r#"
        INSERT INTO tasks (
            id, task_queue_id, identifier, sequence, title, brief, priority, state, review_required
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&task_id)
    .bind(&queue.id)
    .bind(&identifier)
    .bind(sequence)
    .bind(&input.title)
    .bind(&input.brief)
    .bind(&input.priority)
    .bind(&input.state)
    .bind(input.review_required)
    .execute(&mut *tx)
    .await
    .with_context(|| format!("failed to create Task {identifier}"))?;

    for (index, description) in input.acceptance_criteria.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO acceptance_criteria (id, task_id, position, description)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&task_id)
        .bind((index + 1) as i64)
        .bind(description)
        .execute(&mut *tx)
        .await
        .context("failed to create Acceptance Criterion")?;
    }

    for (index, description) in input.validation_items.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO validation_items (id, task_id, position, description)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&task_id)
        .bind((index + 1) as i64)
        .bind(description)
        .execute(&mut *tx)
        .await
        .context("failed to create Validation Item")?;
    }

    for tag in normalized_tags(&input.tags) {
        sqlx::query("INSERT INTO task_tags (task_id, tag) VALUES (?, ?)")
            .bind(&task_id)
            .bind(tag)
            .execute(&mut *tx)
            .await
            .context("failed to create Task Tag")?;
    }

    let conflict_hints = normalized_conflict_hints(&input.conflict_hints);
    for (index, target) in conflict_hints.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO task_conflict_hints (id, task_id, position, target)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&task_id)
        .bind((index + 1) as i64)
        .bind(target)
        .execute(&mut *tx)
        .await
        .context("failed to create Task Conflict Hint")?;
    }

    let payload_json = serde_json::json!({
        "identifier": identifier,
        "queue_key": queue.key,
        "title": input.title,
        "priority": input.priority,
        "state": input.state,
        "review_required": input.review_required,
        "acceptance_criteria_count": input.acceptance_criteria.len(),
        "validation_items_count": input.validation_items.len(),
        "tags": normalized_tags(&input.tags),
        "conflict_hints": conflict_hints,
    })
    .to_string();
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_kind, actor_id, actor_display_name, event_type, subject_type, subject_id, payload_json
        ) VALUES (?, ?, ?, ?, 'task.created', 'task', ?, ?)
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

    get_task_detail(pool, &identifier)
        .await?
        .with_context(|| format!("created Task {identifier} was not found"))
}

pub async fn create_child_task(
    pool: &SqlitePool,
    parent_identifier: &str,
    input: &CreateChildTask,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_child_task_actor(actor)?;
    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let parent = sqlx::query_as::<_, Task>(
        r#"
        SELECT tasks.id, tasks.task_queue_id, task_queues.key AS task_queue_key, tasks.identifier,
               tasks.sequence, tasks.title, tasks.brief, tasks.priority, tasks.state,
               tasks.review_required, tasks.validated_base_commit, tasks.created_at, tasks.updated_at
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.identifier = ?
        "#,
    )
    .bind(parent_identifier)
    .fetch_optional(&mut *tx)
    .await
    .with_context(|| format!("failed to load Task {parent_identifier}"))?
    .with_context(|| format!("Task {parent_identifier} not found"))?;
    let child_input = CreateTask {
        queue_key: parent.task_queue_key.clone(),
        title: input.title.clone(),
        brief: input.brief.clone(),
        priority: input.priority.clone(),
        state: input.state.clone(),
        review_required: input.review_required,
        acceptance_criteria: input.acceptance_criteria.clone(),
        validation_items: input.validation_items.clone(),
        tags: input.tags.clone(),
        conflict_hints: input.conflict_hints.clone(),
    };
    validate_create_task(&child_input)?;

    let sequence: i64 = sqlx::query_scalar(
        r#"
        UPDATE task_queues
        SET next_task_sequence = next_task_sequence + 1,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        RETURNING next_task_sequence - 1
        "#,
    )
    .bind(&parent.task_queue_id)
    .fetch_one(&mut *tx)
    .await
    .context("failed to allocate Child Task Identifier sequence")?;
    let child_task_id = Uuid::new_v4().to_string();
    let child_identifier = format!("{}-{}", parent.task_queue_key, sequence);
    sqlx::query(
        r#"
        INSERT INTO tasks (id, task_queue_id, identifier, sequence, title, brief, priority, state, review_required)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&child_task_id)
    .bind(&parent.task_queue_id)
    .bind(&child_identifier)
    .bind(sequence)
    .bind(&child_input.title)
    .bind(&child_input.brief)
    .bind(&child_input.priority)
    .bind(&child_input.state)
    .bind(child_input.review_required)
    .execute(&mut *tx)
    .await
    .with_context(|| format!("failed to create Child Task {child_identifier}"))?;
    for (index, description) in child_input.acceptance_criteria.iter().enumerate() {
        sqlx::query("INSERT INTO acceptance_criteria (id, task_id, position, description) VALUES (?, ?, ?, ?)")
            .bind(Uuid::new_v4().to_string())
            .bind(&child_task_id)
            .bind((index + 1) as i64)
            .bind(description)
            .execute(&mut *tx)
            .await
            .context("failed to create Child Task Acceptance Criterion")?;
    }
    for (index, description) in child_input.validation_items.iter().enumerate() {
        sqlx::query(
            "INSERT INTO validation_items (id, task_id, position, description) VALUES (?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&child_task_id)
        .bind((index + 1) as i64)
        .bind(description)
        .execute(&mut *tx)
        .await
        .context("failed to create Child Task Validation Item")?;
    }
    for tag in normalized_tags(&child_input.tags) {
        sqlx::query("INSERT INTO task_tags (task_id, tag) VALUES (?, ?)")
            .bind(&child_task_id)
            .bind(tag)
            .execute(&mut *tx)
            .await
            .context("failed to create Child Task Tag")?;
    }
    let child_conflict_hints = normalized_conflict_hints(&child_input.conflict_hints);
    for (index, target) in child_conflict_hints.iter().enumerate() {
        sqlx::query(
            "INSERT INTO task_conflict_hints (id, task_id, position, target) VALUES (?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&child_task_id)
        .bind((index + 1) as i64)
        .bind(target)
        .execute(&mut *tx)
        .await
        .context("failed to create Child Task Conflict Hint")?;
    }
    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task.created",
        "task",
        &child_task_id,
        serde_json::json!({
            "identifier": child_identifier,
            "queue_key": parent.task_queue_key,
            "title": child_input.title,
            "priority": child_input.priority,
            "state": child_input.state,
            "review_required": child_input.review_required,
            "acceptance_criteria_count": child_input.acceptance_criteria.len(),
            "validation_items_count": child_input.validation_items.len(),
            "tags": normalized_tags(&child_input.tags),
            "conflict_hints": child_conflict_hints,
        }),
    )
    .await?;
    sqlx::query("INSERT INTO task_relationships (id, source_task_id, target_task_id, relationship_kind) VALUES (?, ?, ?, 'parent_child')")
        .bind(Uuid::new_v4().to_string())
        .bind(&parent.id)
        .bind(&child_task_id)
        .execute(&mut *tx)
        .await
        .context("failed to create Child Task relationship")?;
    if input.blocks_parent {
        sqlx::query("INSERT INTO task_relationships (id, source_task_id, target_task_id, relationship_kind) VALUES (?, ?, ?, 'blocks')")
            .bind(Uuid::new_v4().to_string())
            .bind(&child_task_id)
            .bind(&parent.id)
            .execute(&mut *tx)
            .await
            .context("failed to create Blocking Task relationship")?;
    }
    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task.child_created",
        "task",
        &parent.id,
        serde_json::json!({
            "parent_identifier": parent_identifier,
            "child_identifier": child_identifier,
            "blocks_parent": input.blocks_parent,
        }),
    )
    .await?;
    tx.commit().await.context("failed to commit transaction")?;

    get_task_detail(pool, &child_identifier)
        .await?
        .with_context(|| format!("created Child Task {child_identifier} was not found"))
}

pub async fn get_task_detail(pool: &SqlitePool, identifier: &str) -> Result<Option<TaskDetail>> {
    let Some(task) = sqlx::query_as::<_, Task>(
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
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to load Task {identifier}"))?
    else {
        return Ok(None);
    };

    let acceptance_criteria = sqlx::query_as::<_, AcceptanceCriterion>(
        r#"
        SELECT id, task_id, position, description, status, waiver_reason
        FROM acceptance_criteria
        WHERE task_id = ?
        ORDER BY position
        "#,
    )
    .bind(&task.id)
    .fetch_all(pool)
    .await
    .context("failed to load Acceptance Criteria")?;

    let validation_items = sqlx::query_as::<_, ValidationItem>(
        r#"
        SELECT id, task_id, position, description, status, waiver_reason
        FROM validation_items
        WHERE task_id = ?
        ORDER BY position
        "#,
    )
    .bind(&task.id)
    .fetch_all(pool)
    .await
    .context("failed to load Validation Items")?;

    let tags = sqlx::query_scalar(
        r#"
        SELECT tag
        FROM task_tags
        WHERE task_id = ?
        ORDER BY tag
        "#,
    )
    .bind(&task.id)
    .fetch_all(pool)
    .await
    .context("failed to load Task Tags")?;

    let workpad_note = sqlx::query_as::<_, WorkpadNote>(
        r#"
        SELECT id, task_id, body, created_at, updated_at
        FROM workpad_notes
        WHERE task_id = ?
        "#,
    )
    .bind(&task.id)
    .fetch_optional(pool)
    .await
    .context("failed to load Workpad Note")?;

    let task_links = sqlx::query_as::<_, TaskLink>(
        r#"
        SELECT id, task_id, kind, target, label, is_primary, created_at, updated_at
        FROM task_links
        WHERE task_id = ?
        ORDER BY is_primary DESC, kind, target
        "#,
    )
    .bind(&task.id)
    .fetch_all(pool)
    .await
    .context("failed to load Task Links")?;

    let conflict_hints = sqlx::query_as::<_, TaskConflictHint>(
        r#"
        SELECT id, task_id, position, target
        FROM task_conflict_hints
        WHERE task_id = ?
        ORDER BY position
        "#,
    )
    .bind(&task.id)
    .fetch_all(pool)
    .await
    .context("failed to load Task Conflict Hints")?;

    let conflict_overlaps = sqlx::query_as::<_, TaskConflictOverlap>(
        r#"
        SELECT
            self_hints.target AS target,
            other_tasks.identifier AS task_identifier,
            other_tasks.title AS title,
            other_tasks.state AS state
        FROM task_conflict_hints AS self_hints
        JOIN task_conflict_hints AS other_hints
          ON other_hints.target = self_hints.target
         AND other_hints.task_id != self_hints.task_id
        JOIN tasks AS other_tasks ON other_tasks.id = other_hints.task_id
        WHERE self_hints.task_id = ?
          AND other_tasks.task_queue_id = ?
          AND other_tasks.state IN ('ready', 'in_progress')
        ORDER BY self_hints.position, other_tasks.identifier
        "#,
    )
    .bind(&task.id)
    .bind(&task.task_queue_id)
    .fetch_all(pool)
    .await
    .context("failed to load Task Conflict overlaps")?;

    let latest_rework_outcome = sqlx::query_as::<_, TaskContextIntegrationOutcome>(
        r#"
        SELECT
            id,
            agent_run_id,
            outcome_kind,
            reason_code,
            final_commit,
            pre_merge_head,
            message,
            retryable,
            retry_attempt,
            next_retry_at,
            created_at
        FROM integration_outcomes
        WHERE task_id = ?
          AND outcome_kind != 'success'
        ORDER BY created_at DESC, rowid DESC
        LIMIT 1
        "#,
    )
    .bind(&task.id)
    .fetch_optional(pool)
    .await
    .context("failed to load latest Rework Integration Outcome")?;
    let latest_rework_run = if latest_rework_outcome.is_none() {
        sqlx::query_as::<_, TaskContextRunFailure>(
            r#"
            SELECT
                id AS agent_run_id,
                outcome AS outcome,
                failure_reason,
                failure_reason_code,
                finished_at
            FROM agent_runs
            WHERE task_id = ?
              AND outcome IS NOT NULL
              AND outcome != 'completed'
            ORDER BY finished_at DESC, created_at DESC, id DESC
            LIMIT 1
            "#,
        )
        .bind(&task.id)
        .fetch_optional(pool)
        .await
        .context("failed to load latest Rework Agent Run failure")?
    } else {
        None
    };
    let latest_rework_reason_code = latest_rework_outcome
        .as_ref()
        .and_then(|outcome| outcome.reason_code.clone())
        .or_else(|| {
            latest_rework_run
                .as_ref()
                .and_then(|run| run.failure_reason_code.clone())
        });
    let latest_rework_reason = latest_rework_outcome
        .as_ref()
        .and_then(|outcome| outcome.message.clone())
        .or_else(|| {
            latest_rework_run
                .as_ref()
                .and_then(|run| run.failure_reason.clone())
        });

    Ok(Some(TaskDetail {
        task,
        acceptance_criteria,
        validation_items,
        tags,
        workpad_note,
        task_links,
        conflict_hints,
        conflict_overlaps,
        latest_rework_reason_code,
        latest_rework_reason,
    }))
}

pub async fn get_task_context_bundle(
    pool: &SqlitePool,
    identifier: &str,
) -> Result<Option<TaskContextBundle>> {
    let Some(task) = get_task_detail(pool, identifier).await? else {
        return Ok(None);
    };
    let queue = get_task_queue(pool, &task.task.task_queue_key)
        .await?
        .with_context(|| format!("Task Queue {} not found", task.task.task_queue_key))?;
    let local_worktree = primary_task_link_target(&task.task_links, "local_worktree");
    let task_branch = primary_task_link_target(&task.task_links, "task_branch");

    let agent_runs = sqlx::query_as::<_, TaskContextAgentRun>(
        r#"
        SELECT
            id,
            worker_actor_kind,
            worker_actor_id,
            worker_actor_display_name,
            worker_id,
            launcher_kind,
            lease_expires_at,
            last_heartbeat_at,
            outcome,
            failure_reason,
            failure_reason_code,
            created_at,
            finished_at,
            outcome IS NULL AND lease_expires_at > CURRENT_TIMESTAMP AS is_active
        FROM agent_runs
        WHERE task_id = ?
        ORDER BY created_at DESC, id DESC
        LIMIT 5
        "#,
    )
    .bind(&task.task.id)
    .fetch_all(pool)
    .await
    .context("failed to load recent Agent Runs for Task context bundle")?;

    let latest_failure = sqlx::query_as::<_, TaskContextRunFailure>(
        r#"
        SELECT
            id AS agent_run_id,
            outcome AS outcome,
            failure_reason,
            failure_reason_code,
            finished_at
        FROM agent_runs
        WHERE task_id = ?
          AND outcome IS NOT NULL
          AND outcome != 'completed'
        ORDER BY finished_at DESC, created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(&task.task.id)
    .fetch_optional(pool)
    .await
    .context("failed to load latest Agent Run failure for Task context bundle")?;

    let latest_integration_outcome = sqlx::query_as::<_, TaskContextIntegrationOutcome>(
        r#"
        SELECT
            id,
            agent_run_id,
            outcome_kind,
            reason_code,
            final_commit,
            pre_merge_head,
            message,
            retryable,
            retry_attempt,
            next_retry_at,
            created_at
        FROM integration_outcomes
        WHERE task_id = ?
        ORDER BY created_at DESC, rowid DESC
        LIMIT 1
        "#,
    )
    .bind(&task.task.id)
    .fetch_optional(pool)
    .await
    .context("failed to load latest Integration Outcome for Task context bundle")?;

    Ok(Some(TaskContextBundle {
        task,
        queue: TaskContextQueue {
            key: queue.key,
            name: queue.name,
            delivery_backend: queue.delivery_backend.clone(),
            main_branch: queue.main_branch.clone(),
            managed_source_repository: queue.managed_source_repository.clone(),
            worktree_root: queue.worktree_root.clone(),
            branch_template: queue.branch_template.clone(),
            queue_concurrency_limit: queue.queue_concurrency_limit,
        },
        local_workflow: TaskLocalWorkflowContext {
            local_worktree,
            task_branch,
            main_branch: queue.main_branch,
            managed_source_repository: queue.managed_source_repository,
            worktree_root: queue.worktree_root,
            branch_template: queue.branch_template,
            delivery_backend: queue.delivery_backend,
        },
        agent_runs,
        latest_failure,
        latest_integration_outcome,
    }))
}

fn primary_task_link_target(task_links: &[TaskLink], kind: &str) -> Option<String> {
    task_links
        .iter()
        .find(|link| link.kind == kind && link.is_primary)
        .or_else(|| task_links.iter().find(|link| link.kind == kind))
        .map(|link| link.target.clone())
}

pub async fn upsert_task_link(
    pool: &SqlitePool,
    identifier: &str,
    input: &UpsertTaskLink,
    actor: &Actor,
) -> Result<TaskDetail> {
    with_sqlite_write_retry(|| upsert_task_link_once(pool, identifier, input, actor)).await
}

async fn upsert_task_link_once(
    pool: &SqlitePool,
    identifier: &str,
    input: &UpsertTaskLink,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_actor(actor)?;
    ensure_not_blank("Task Link kind", &input.kind)?;
    ensure_not_blank("Task Link target", &input.target)?;
    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let task_id: String = sqlx::query_scalar("SELECT id FROM tasks WHERE identifier = ?")
        .bind(identifier)
        .fetch_optional(&mut *tx)
        .await
        .with_context(|| format!("failed to load Task {identifier}"))?
        .with_context(|| format!("Task {identifier} not found"))?;

    if input.is_primary {
        sqlx::query("UPDATE task_links SET is_primary = 0, updated_at = CURRENT_TIMESTAMP WHERE task_id = ?")
            .bind(&task_id)
            .execute(&mut *tx)
            .await
            .context("failed to clear Primary Handoff Link")?;
    }

    let link_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO task_links (id, task_id, kind, target, label, is_primary)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(task_id, kind, target) DO UPDATE SET
            label = excluded.label,
            is_primary = excluded.is_primary,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(&link_id)
    .bind(&task_id)
    .bind(&input.kind)
    .bind(&input.target)
    .bind(&input.label)
    .bind(input.is_primary)
    .execute(&mut *tx)
    .await
    .context("failed to upsert Task Link")?;

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task_link.upserted",
        "task",
        &task_id,
        serde_json::json!({
            "identifier": identifier,
            "kind": input.kind,
            "target": input.target,
            "label": input.label,
            "is_primary": input.is_primary,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;
    get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("updated Task {identifier} was not found"))
}

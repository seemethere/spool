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

pub async fn status_by_queue_and_state(pool: &SqlitePool) -> Result<Vec<QueueStatus>> {
    sqlx::query_as::<_, QueueStatus>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            task_queues.name AS queue_name,
            task_queues.queue_concurrency_limit AS queue_concurrency_limit,
            COALESCE(tasks.state, 'none') AS state,
            COUNT(tasks.id) AS task_count,
            (
                SELECT COUNT(*) FROM tasks ready_tasks
                WHERE ready_tasks.task_queue_id = task_queues.id
                  AND ready_tasks.state = 'ready'
            ) AS ready_tasks,
            (
                SELECT COUNT(*) FROM tasks integrating_tasks
                WHERE integrating_tasks.task_queue_id = task_queues.id
                  AND integrating_tasks.state = 'integrating'
            ) AS integrating_tasks,
            (
                SELECT COUNT(*) FROM agent_runs
                WHERE agent_runs.task_queue_id = task_queues.id
                  AND agent_runs.outcome IS NULL
                  AND agent_runs.lease_expires_at > CURRENT_TIMESTAMP
            ) AS active_agent_runs,
            (
                SELECT COUNT(*) FROM agent_runs
                JOIN tasks active_tasks ON active_tasks.id = agent_runs.task_id
                WHERE agent_runs.task_queue_id = task_queues.id
                  AND active_tasks.state = 'integrating'
                  AND agent_runs.outcome IS NULL
                  AND agent_runs.lease_expires_at > CURRENT_TIMESTAMP
            ) AS active_integrating_agent_runs,
            (
                SELECT COUNT(*) FROM task_retry_holds
                JOIN tasks held_tasks ON held_tasks.id = task_retry_holds.task_id
                WHERE held_tasks.task_queue_id = task_queues.id
                  AND task_retry_holds.hold_until > CURRENT_TIMESTAMP
            ) AS active_retry_holds
        FROM task_queues
        LEFT JOIN tasks ON tasks.task_queue_id = task_queues.id
        GROUP BY task_queues.id, task_queues.key, task_queues.name, task_queues.queue_concurrency_limit, tasks.state
        ORDER BY task_queues.key, tasks.state
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to load Tasker status")
}

pub async fn active_agent_runs_for_status(pool: &SqlitePool) -> Result<Vec<ActiveAgentRunStatus>> {
    sqlx::query_as::<_, ActiveAgentRunStatus>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS task_identifier,
            tasks.title AS task_title,
            tasks.state AS task_state,
            agent_runs.id AS agent_run_id,
            agent_runs.launcher_kind AS launcher_kind,
            agent_runs.worker_id AS worker_id,
            agent_runs.lease_expires_at AS lease_expires_at
        FROM agent_runs
        JOIN tasks ON tasks.id = agent_runs.task_id
        JOIN task_queues ON task_queues.id = agent_runs.task_queue_id
        WHERE agent_runs.outcome IS NULL
          AND agent_runs.lease_expires_at > CURRENT_TIMESTAMP
        ORDER BY task_queues.key, tasks.identifier, agent_runs.created_at, agent_runs.id
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to load active Agent Runs for status")
}

pub async fn tasks_for_status_by_states(
    pool: &SqlitePool,
    states: &[&str],
) -> Result<Vec<TaskStatusSummary>> {
    if states.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = sqlx::QueryBuilder::new(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS identifier,
            tasks.title AS title,
            tasks.state AS state,
            tasks.priority AS priority,
            task_queues.main_branch AS main_branch,
            (
                SELECT task_links.target FROM task_links
                WHERE task_links.task_id = tasks.id AND task_links.kind = 'local_worktree'
                ORDER BY task_links.is_primary DESC, task_links.created_at DESC, task_links.id DESC
                LIMIT 1
            ) AS local_worktree,
            (
                SELECT task_links.target FROM task_links
                WHERE task_links.task_id = tasks.id AND task_links.kind = 'task_branch'
                ORDER BY task_links.is_primary DESC, task_links.created_at DESC, task_links.id DESC
                LIMIT 1
            ) AS task_branch,
            COALESCE(
                (
                    SELECT integration_outcomes.reason_code FROM integration_outcomes
                    WHERE integration_outcomes.task_id = tasks.id
                      AND integration_outcomes.outcome_kind != 'success'
                    ORDER BY integration_outcomes.created_at DESC, integration_outcomes.rowid DESC
                    LIMIT 1
                ),
                (
                    SELECT agent_runs.failure_reason_code FROM agent_runs
                    WHERE agent_runs.task_id = tasks.id
                      AND agent_runs.outcome IS NOT NULL
                      AND agent_runs.outcome != 'completed'
                    ORDER BY agent_runs.finished_at DESC, agent_runs.created_at DESC, agent_runs.id DESC
                    LIMIT 1
                )
            ) AS latest_rework_reason_code,
            COALESCE(
                (
                    SELECT integration_outcomes.message FROM integration_outcomes
                    WHERE integration_outcomes.task_id = tasks.id
                      AND integration_outcomes.outcome_kind != 'success'
                    ORDER BY integration_outcomes.created_at DESC, integration_outcomes.rowid DESC
                    LIMIT 1
                ),
                (
                    SELECT agent_runs.failure_reason FROM agent_runs
                    WHERE agent_runs.task_id = tasks.id
                      AND agent_runs.outcome IS NOT NULL
                      AND agent_runs.outcome != 'completed'
                    ORDER BY agent_runs.finished_at DESC, agent_runs.created_at DESC, agent_runs.id DESC
                    LIMIT 1
                )
            ) AS latest_rework_reason
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.state IN (
        "#,
    );
    let mut separated = query.separated(", ");
    for state in states {
        separated.push_bind(*state);
    }
    separated.push_unseparated(")");
    query.push(" ORDER BY task_queues.key, tasks.state, tasks.priority, tasks.identifier");
    query
        .build_query_as::<TaskStatusSummary>()
        .fetch_all(pool)
        .await
        .context("failed to load Task summaries for status")
}

pub async fn active_retry_holds_for_status(
    pool: &SqlitePool,
) -> Result<Vec<ActiveRetryHoldStatus>> {
    sqlx::query_as::<_, ActiveRetryHoldStatus>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS task_identifier,
            task_retry_holds.hold_until AS hold_until,
            task_retry_holds.reason AS reason,
            agent_runs.failure_reason_code AS failure_reason_code
        FROM task_retry_holds
        JOIN tasks ON tasks.id = task_retry_holds.task_id
        LEFT JOIN agent_runs ON agent_runs.id = task_retry_holds.agent_run_id
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE task_retry_holds.hold_until > CURRENT_TIMESTAMP
        ORDER BY task_queues.key, tasks.identifier
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to load active Retry Holds for status")
}

pub async fn integration_retries_for_status(
    pool: &SqlitePool,
) -> Result<Vec<IntegrationRetryStatus>> {
    sqlx::query_as::<_, IntegrationRetryStatus>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS task_identifier,
            tasks.title AS task_title,
            COALESCE(latest.reason_code, 'unknown_legacy') AS reason_code,
            latest.retryable AS retryable,
            latest.retry_attempt AS retry_attempt,
            latest.next_retry_at AS next_retry_at,
            latest.message AS reason
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        JOIN integration_outcomes latest ON latest.id = (
            SELECT integration_outcomes.id FROM integration_outcomes
            WHERE integration_outcomes.task_id = tasks.id
            ORDER BY integration_outcomes.created_at DESC, integration_outcomes.rowid DESC
            LIMIT 1
        )
        WHERE tasks.state = 'integrating'
          AND latest.outcome_kind = 'operational_failure'
        ORDER BY task_queues.key, tasks.identifier
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to load Integration retry status")
}

pub async fn due_integration_retries(
    pool: &SqlitePool,
    queue_key: &str,
) -> Result<Vec<IntegrationRetryStatus>> {
    sqlx::query_as::<_, IntegrationRetryStatus>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS task_identifier,
            tasks.title AS task_title,
            COALESCE(latest.reason_code, 'unknown_legacy') AS reason_code,
            latest.retryable AS retryable,
            latest.retry_attempt AS retry_attempt,
            latest.next_retry_at AS next_retry_at,
            latest.message AS reason
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        JOIN integration_outcomes latest ON latest.id = (
            SELECT integration_outcomes.id FROM integration_outcomes
            WHERE integration_outcomes.task_id = tasks.id
            ORDER BY integration_outcomes.created_at DESC, integration_outcomes.rowid DESC
            LIMIT 1
        )
        WHERE task_queues.key = ?
          AND tasks.state = 'integrating'
          AND latest.outcome_kind = 'operational_failure'
          AND latest.retryable = 1
          AND latest.next_retry_at IS NOT NULL
          AND latest.next_retry_at <= CURRENT_TIMESTAMP
          AND NOT EXISTS (
              SELECT 1 FROM agent_runs
              WHERE agent_runs.task_id = tasks.id
                AND agent_runs.outcome IS NULL
                AND agent_runs.lease_expires_at > CURRENT_TIMESTAMP
          )
        ORDER BY latest.next_retry_at, tasks.identifier
        "#,
    )
    .bind(queue_key)
    .fetch_all(pool)
    .await
    .context("failed to load due Integration retries")
}

pub async fn task_conflict_groups_for_status(pool: &SqlitePool) -> Result<Vec<TaskConflictGroup>> {
    sqlx::query_as::<_, TaskConflictGroup>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            task_conflict_hints.target AS target,
            COUNT(DISTINCT tasks.id) AS task_count,
            group_concat(tasks.identifier || ' (' || tasks.state || ')', ', ') AS tasks
        FROM task_conflict_hints
        JOIN tasks ON tasks.id = task_conflict_hints.task_id
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.state IN ('ready', 'in_progress')
        GROUP BY task_queues.key, task_conflict_hints.target
        HAVING COUNT(DISTINCT tasks.id) > 1
        ORDER BY task_queues.key, task_conflict_hints.target
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to load Task conflict hints for status")
}

pub async fn merge_queue_tasks(
    pool: &SqlitePool,
    queue_key: Option<&str>,
) -> Result<Vec<MergeQueueTask>> {
    sqlx::query_as::<_, MergeQueueTask>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS task_identifier,
            tasks.title AS title,
            (
                SELECT task_links.target FROM task_links
                WHERE task_links.task_id = tasks.id AND task_links.kind = 'task_branch'
                ORDER BY task_links.is_primary DESC, task_links.created_at DESC, task_links.id DESC
                LIMIT 1
            ) AS task_branch,
            (
                SELECT task_links.target FROM task_links
                WHERE task_links.task_id = tasks.id AND task_links.kind = 'local_worktree'
                ORDER BY task_links.is_primary DESC, task_links.created_at DESC, task_links.id DESC
                LIMIT 1
            ) AS local_worktree,
            task_queues.main_branch AS main_branch,
            (
                SELECT agent_runs.id FROM agent_runs
                WHERE agent_runs.task_id = tasks.id
                ORDER BY agent_runs.created_at DESC, agent_runs.id DESC
                LIMIT 1
            ) AS latest_agent_run_id,
            (
                SELECT COALESCE(agent_runs.outcome, 'active') FROM agent_runs
                WHERE agent_runs.task_id = tasks.id
                ORDER BY agent_runs.created_at DESC, agent_runs.id DESC
                LIMIT 1
            ) AS latest_agent_run_outcome,
            (
                SELECT COUNT(*) FROM acceptance_criteria
                WHERE acceptance_criteria.task_id = tasks.id
                  AND acceptance_criteria.status NOT IN ('satisfied', 'waived')
            ) AS pending_acceptance_criteria,
            (
                SELECT COUNT(*) FROM validation_items
                WHERE validation_items.task_id = tasks.id
                  AND validation_items.status NOT IN ('passed', 'waived')
            ) AS pending_validation_items,
            (
                SELECT COUNT(*) FROM validation_items
                WHERE validation_items.task_id = tasks.id
                  AND validation_items.status = 'failed'
            ) AS failed_validation_items
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.state = 'integrating'
          AND (? IS NULL OR task_queues.key = ?)
        ORDER BY task_queues.key, tasks.identifier
        "#,
    )
    .bind(queue_key)
    .bind(queue_key)
    .fetch_all(pool)
    .await
    .context("failed to load Manual Dogfood Merge queue")
}

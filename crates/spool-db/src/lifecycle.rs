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

pub(crate) async fn expire_stale_agent_runs(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<()> {
    let expired = sqlx::query_as::<_, AgentRun>(
        r#"
        UPDATE agent_runs
        SET outcome = 'expired', finished_at = CURRENT_TIMESTAMP, failure_reason = 'Claim Lease expired', failure_reason_code = 'claim_lease_expired'
        WHERE outcome IS NULL AND lease_expires_at <= CURRENT_TIMESTAMP
        RETURNING id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                  worker_actor_display_name, worker_id, launcher_kind, lease_expires_at,
                  last_heartbeat_at, outcome, failure_reason, failure_reason_code, created_at, finished_at
        "#,
    )
    .fetch_all(&mut **tx)
    .await
    .context("failed to expire stale Agent Runs")?;

    for run in expired {
        sqlx::query(
            r#"
            INSERT INTO task_retry_holds (task_id, agent_run_id, hold_until, reason)
            VALUES (?, ?, datetime('now', '+60 seconds'), 'Claim Lease expired')
            ON CONFLICT(task_id) DO UPDATE SET
                agent_run_id = excluded.agent_run_id,
                hold_until = excluded.hold_until,
                reason = excluded.reason,
                created_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&run.task_id)
        .bind(&run.id)
        .execute(&mut **tx)
        .await
        .context("failed to create Retry Hold for expired Agent Run")?;
        let actor = Actor {
            kind: run.worker_actor_kind.clone(),
            id: run.worker_actor_id.clone(),
            display_name: run.worker_actor_display_name.clone(),
        };
        append_audit_event_in_tx(
            tx,
            &actor,
            "task.retry_hold_created",
            "task",
            &run.task_id,
            serde_json::json!({
                "agent_run_id": run.id,
                "hold_seconds": 60,
                "reason": "Claim Lease expired",
                "failure_reason_code": run.failure_reason_code,
            }),
        )
        .await?;
        append_audit_event_in_tx(
            tx,
            &actor,
            "agent_run.expired",
            "agent_run",
            &run.id,
            serde_json::json!({ "reason": "Claim Lease expired", "failure_reason_code": run.failure_reason_code }),
        )
        .await?;
        sqlx::query(
            r#"
            INSERT INTO agent_run_metrics (
                agent_run_id, derivation_version, duration_ms, launcher_kind, final_status, warnings_json
            )
            SELECT
                id,
                ?,
                CAST((julianday(finished_at) - julianday(created_at)) * 86400000 AS INTEGER),
                launcher_kind,
                outcome,
                '["Launcher Session Data not recorded"]'
            FROM agent_runs
            WHERE id = ?
            ON CONFLICT(agent_run_id) DO UPDATE SET
                derivation_version = excluded.derivation_version,
                duration_ms = excluded.duration_ms,
                launcher_kind = excluded.launcher_kind,
                final_status = excluded.final_status,
                warnings_json = excluded.warnings_json,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(crate::CURRENT_AGENT_RUN_METRICS_DERIVATION_VERSION)
        .bind(&run.id)
        .execute(&mut **tx)
        .await
        .context("failed to persist expired Agent Run metrics")?;
    }

    Ok(())
}

pub(crate) fn agent_run_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT agent_runs.id, agent_runs.task_id, agent_runs.task_queue_id, agent_runs.worker_actor_kind, agent_runs.worker_actor_id, agent_runs.worker_actor_display_name, agent_runs.worker_id, agent_runs.launcher_kind, agent_runs.lease_expires_at, agent_runs.last_heartbeat_at, agent_runs.outcome, agent_runs.failure_reason, agent_runs.failure_reason_code, agent_runs.created_at, agent_runs.finished_at FROM agent_runs {where_clause}"
    )
}

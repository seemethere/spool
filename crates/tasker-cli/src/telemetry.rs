use anyhow::{Context, Result};
use sqlx::{FromRow, SqlitePool};
use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryOptions {
    pub queue: String,
    pub slow_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetrySummary {
    pub queue: String,
    pub total_runs: usize,
    pub duplicate_tasks: Vec<DuplicateTaskRun>,
    pub post_integrating_runs: Vec<TelemetryRun>,
    pub failed_or_timed_out_runs: Vec<TelemetryRun>,
    pub completed_duration_seconds: Vec<i64>,
    pub slowest_completed_runs: Vec<TelemetryRun>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateTaskRun {
    pub task_identifier: String,
    pub task_title: String,
    pub run_count: usize,
    pub wasted_runs: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryRun {
    pub task_identifier: String,
    pub task_title: String,
    pub agent_run_id: String,
    pub outcome: Option<String>,
    pub failure_reason: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub duration_seconds: Option<i64>,
}

#[derive(Debug, FromRow)]
struct TelemetryRunRow {
    task_identifier: String,
    task_title: String,
    agent_run_id: String,
    outcome: Option<String>,
    failure_reason: Option<String>,
    created_at: String,
    finished_at: Option<String>,
    integrating_at: Option<String>,
    duration_seconds: Option<i64>,
}

pub async fn summarize_agent_runs(
    pool: &SqlitePool,
    options: &TelemetryOptions,
) -> Result<TelemetrySummary> {
    let rows = sqlx::query_as::<_, TelemetryRunRow>(
        r#"
        WITH first_integrating AS (
            SELECT subject_id AS task_id, MIN(created_at) AS integrating_at
            FROM audit_events
            WHERE subject_type = 'task'
              AND event_type = 'task.state_transitioned'
              AND payload_json LIKE '%"to":"integrating"%'
            GROUP BY subject_id
        )
        SELECT
            tasks.identifier AS task_identifier,
            tasks.title AS task_title,
            agent_runs.id AS agent_run_id,
            agent_runs.outcome AS outcome,
            agent_runs.failure_reason AS failure_reason,
            agent_runs.created_at AS created_at,
            agent_runs.finished_at AS finished_at,
            first_integrating.integrating_at AS integrating_at,
            CASE
                WHEN COALESCE(launcher_session_data.finished_at, agent_runs.finished_at) IS NOT NULL
                 AND COALESCE(launcher_session_data.started_at, agent_runs.created_at) IS NOT NULL
                THEN CAST(strftime('%s', COALESCE(launcher_session_data.finished_at, agent_runs.finished_at)) AS INTEGER)
                   - CAST(strftime('%s', COALESCE(launcher_session_data.started_at, agent_runs.created_at)) AS INTEGER)
                ELSE NULL
            END AS duration_seconds
        FROM agent_runs
        JOIN tasks ON tasks.id = agent_runs.task_id
        JOIN task_queues ON task_queues.id = agent_runs.task_queue_id
        LEFT JOIN launcher_session_data ON launcher_session_data.agent_run_id = agent_runs.id
        LEFT JOIN first_integrating ON first_integrating.task_id = tasks.id
        WHERE task_queues.key = ?
        ORDER BY tasks.sequence, agent_runs.created_at, agent_runs.id
        "#,
    )
    .bind(&options.queue)
    .fetch_all(pool)
    .await
    .with_context(|| {
        format!(
            "failed to summarize Agent Run telemetry for Task Queue {}",
            options.queue
        )
    })?;

    let mut duplicate_tasks = Vec::new();
    let mut current_task: Option<(String, String, usize)> = None;
    for row in &rows {
        match &mut current_task {
            Some((identifier, _, count)) if identifier == &row.task_identifier => *count += 1,
            Some((identifier, title, count)) => {
                if *count > 1 {
                    duplicate_tasks.push(DuplicateTaskRun {
                        task_identifier: identifier.clone(),
                        task_title: title.clone(),
                        run_count: *count,
                        wasted_runs: *count - 1,
                    });
                }
                current_task = Some((row.task_identifier.clone(), row.task_title.clone(), 1));
            }
            None => current_task = Some((row.task_identifier.clone(), row.task_title.clone(), 1)),
        }
    }
    if let Some((identifier, title, count)) = current_task {
        if count > 1 {
            duplicate_tasks.push(DuplicateTaskRun {
                task_identifier: identifier,
                task_title: title,
                run_count: count,
                wasted_runs: count - 1,
            });
        }
    }

    let post_integrating_runs = rows
        .iter()
        .filter(|row| {
            row.integrating_at
                .as_ref()
                .is_some_and(|integrating_at| row.created_at > *integrating_at)
        })
        .map(run_from_row)
        .collect();

    let failed_or_timed_out_runs = rows
        .iter()
        .filter(|row| {
            row.outcome.as_deref() == Some("failed")
                || row.outcome.as_deref() == Some("expired")
                || row
                    .failure_reason
                    .as_ref()
                    .is_some_and(|reason| reason.to_ascii_lowercase().contains("timeout"))
                || row
                    .failure_reason
                    .as_ref()
                    .is_some_and(|reason| reason.to_ascii_lowercase().contains("timed out"))
        })
        .map(run_from_row)
        .collect();

    let mut completed_runs_with_duration: Vec<_> = rows
        .iter()
        .filter(|row| row.outcome.as_deref() == Some("completed"))
        .filter_map(|row| {
            row.duration_seconds
                .map(|duration| (duration, run_from_row(row)))
        })
        .collect();
    completed_runs_with_duration.sort_by(|(left, left_run), (right, right_run)| {
        right
            .cmp(left)
            .then_with(|| left_run.task_identifier.cmp(&right_run.task_identifier))
            .then_with(|| left_run.agent_run_id.cmp(&right_run.agent_run_id))
    });

    let completed_duration_seconds = completed_runs_with_duration
        .iter()
        .map(|(duration, _)| *duration)
        .collect();
    let slowest_completed_runs = completed_runs_with_duration
        .into_iter()
        .take(options.slow_limit)
        .map(|(_, run)| run)
        .collect();

    Ok(TelemetrySummary {
        queue: options.queue.clone(),
        total_runs: rows.len(),
        duplicate_tasks,
        post_integrating_runs,
        failed_or_timed_out_runs,
        completed_duration_seconds,
        slowest_completed_runs,
    })
}

fn run_from_row(row: &TelemetryRunRow) -> TelemetryRun {
    TelemetryRun {
        task_identifier: row.task_identifier.clone(),
        task_title: row.task_title.clone(),
        agent_run_id: row.agent_run_id.clone(),
        outcome: row.outcome.clone(),
        failure_reason: row.failure_reason.clone(),
        created_at: row.created_at.clone(),
        finished_at: row.finished_at.clone(),
        duration_seconds: row.duration_seconds,
    }
}

pub fn render_summary(summary: &TelemetrySummary) -> String {
    let mut output = String::new();
    writeln!(output, "Agent Run telemetry summary").expect("write string");
    writeln!(output, "Task Queue: {}", summary.queue).expect("write string");
    writeln!(output, "total Agent Runs: {}", summary.total_runs).expect("write string");
    writeln!(
        output,
        "duplicate Agent Run waste: {} wasted run(s) across {} Task(s)",
        summary
            .duplicate_tasks
            .iter()
            .map(|task| task.wasted_runs)
            .sum::<usize>(),
        summary.duplicate_tasks.len()
    )
    .expect("write string");
    for task in &summary.duplicate_tasks {
        writeln!(
            output,
            "  {} - {}: {} run(s), {} duplicate/wasted",
            task.task_identifier, task.task_title, task.run_count, task.wasted_runs
        )
        .expect("write string");
    }

    writeln!(
        output,
        "post-Integrating Agent Runs: {}",
        summary.post_integrating_runs.len()
    )
    .expect("write string");
    for run in &summary.post_integrating_runs {
        writeln!(
            output,
            "  {} - {}: {} started_at={} outcome={}",
            run.task_identifier,
            run.task_title,
            run.agent_run_id,
            run.created_at,
            run.outcome.as_deref().unwrap_or("active")
        )
        .expect("write string");
    }

    writeln!(
        output,
        "failed/timed-out Agent Runs: {}",
        summary.failed_or_timed_out_runs.len()
    )
    .expect("write string");
    for run in &summary.failed_or_timed_out_runs {
        writeln!(
            output,
            "  {} - {}: {} outcome={} reason={}",
            run.task_identifier,
            run.task_title,
            run.agent_run_id,
            run.outcome.as_deref().unwrap_or("active"),
            run.failure_reason.as_deref().unwrap_or("none")
        )
        .expect("write string");
    }

    if summary.completed_duration_seconds.is_empty() {
        writeln!(
            output,
            "completed run duration: no completed runs with duration data"
        )
        .expect("write string");
    } else {
        let total: i64 = summary.completed_duration_seconds.iter().sum();
        let average = total as f64 / summary.completed_duration_seconds.len() as f64;
        writeln!(
            output,
            "completed run duration: average {:.1}s across {} run(s)",
            average,
            summary.completed_duration_seconds.len()
        )
        .expect("write string");
    }

    writeln!(output, "slowest completed Agent Runs:").expect("write string");
    if summary.slowest_completed_runs.is_empty() {
        writeln!(output, "  none").expect("write string");
    } else {
        for run in &summary.slowest_completed_runs {
            writeln!(
                output,
                "  {} - {}: {} duration={}s finished_at={}",
                run.task_identifier,
                run.task_title,
                run.agent_run_id,
                run.duration_seconds.unwrap_or_default(),
                run.finished_at.as_deref().unwrap_or("unknown")
            )
            .expect("write string");
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn telemetry_detects_duplicate_runs_and_post_integrating_runs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pool = tasker_db::connect(&temp.path().join("tasker.db"))
            .await
            .expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        seed_queue_and_task(&pool, "TASK", "Repeated work")
            .await
            .expect("seed");
        let (task_id, queue_id) = ids(&pool, "TASK-1").await;
        insert_run(
            &pool,
            &task_id,
            &queue_id,
            "run-1",
            "2026-01-01 00:00:00",
            "2026-01-01 00:01:00",
            Some("completed"),
            None,
        )
        .await;
        insert_integrating_event(&pool, &task_id, "2026-01-01 00:02:00").await;
        insert_run(
            &pool,
            &task_id,
            &queue_id,
            "run-2",
            "2026-01-01 00:03:00",
            "2026-01-01 00:04:00",
            Some("completed"),
            None,
        )
        .await;

        let summary = summarize_agent_runs(
            &pool,
            &TelemetryOptions {
                queue: "TASK".to_string(),
                slow_limit: 5,
            },
        )
        .await
        .expect("summary");

        assert_eq!(summary.duplicate_tasks.len(), 1);
        assert_eq!(summary.duplicate_tasks[0].task_identifier, "TASK-1");
        assert_eq!(summary.duplicate_tasks[0].wasted_runs, 1);
        assert_eq!(summary.post_integrating_runs.len(), 1);
        assert_eq!(summary.post_integrating_runs[0].agent_run_id, "run-2");
        let rendered = render_summary(&summary);
        assert!(rendered.contains("TASK-1 - Repeated work"));
        assert!(rendered.contains("post-Integrating Agent Runs: 1"));
    }

    #[tokio::test]
    async fn telemetry_renders_duration_summary_with_normalized_fallback() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pool = tasker_db::connect(&temp.path().join("tasker.db"))
            .await
            .expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        seed_queue_and_task(&pool, "TASK", "Durations")
            .await
            .expect("seed one");
        tasker_db::create_task(
            &pool,
            &tasker_db::CreateTask {
                queue_key: "TASK".to_string(),
                title: "Failed work".to_string(),
                brief: "Brief".to_string(),
                priority: "normal".to_string(),
                state: "ready".to_string(),
                review_required: false,
                acceptance_criteria: vec!["accepted".to_string()],
                validation_items: vec!["validated".to_string()],
                tags: vec![],
                conflict_hints: vec![],
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("seed two");
        let (task_id, queue_id) = ids(&pool, "TASK-1").await;
        insert_run(
            &pool,
            &task_id,
            &queue_id,
            "fallback-run",
            "2026-01-01 00:00:00",
            "2026-01-01 00:01:00",
            Some("completed"),
            None,
        )
        .await;
        insert_run(
            &pool,
            &task_id,
            &queue_id,
            "normalized-run",
            "2026-01-01 00:00:00",
            "2026-01-01 00:20:00",
            Some("completed"),
            None,
        )
        .await;
        sqlx::query("INSERT INTO launcher_session_data (agent_run_id, launcher_kind, started_at, finished_at) VALUES ('normalized-run', 'pi', '2026-01-01 00:05:00', '2026-01-01 00:07:00')")
            .execute(&pool)
            .await
            .expect("session data");
        let (failed_task_id, failed_queue_id) = ids(&pool, "TASK-2").await;
        insert_run(
            &pool,
            &failed_task_id,
            &failed_queue_id,
            "failed-run",
            "2026-01-01 00:08:00",
            "2026-01-01 00:09:00",
            Some("failed"),
            Some("launcher timed out"),
        )
        .await;

        let summary = summarize_agent_runs(
            &pool,
            &TelemetryOptions {
                queue: "TASK".to_string(),
                slow_limit: 5,
            },
        )
        .await
        .expect("summary");
        let rendered = render_summary(&summary);

        assert_eq!(summary.completed_duration_seconds, vec![120, 60]);
        assert_eq!(
            summary.slowest_completed_runs[0].agent_run_id,
            "normalized-run"
        );
        assert_eq!(
            summary.failed_or_timed_out_runs[0].agent_run_id,
            "failed-run"
        );
        assert!(rendered.contains("completed run duration: average 90.0s across 2 run(s)"));
        assert!(rendered.contains("normalized-run duration=120s"));
        assert!(rendered.contains("TASK-2 - Failed work"));
    }

    async fn seed_queue_and_task(pool: &SqlitePool, queue: &str, title: &str) -> Result<()> {
        tasker_db::create_task_queue(
            pool,
            &tasker_db::CreateTaskQueue {
                key: queue.to_string(),
                name: "Tasker".to_string(),
                managed_source_repository: "/tmp/repo".to_string(),
                main_branch: "main".to_string(),
                worktree_root: "/tmp/worktrees".to_string(),
                branch_template: "tasker/{task_identifier}".to_string(),
                done_worktree_retention: false,
                queue_concurrency_limit: None,
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await?;
        tasker_db::create_task(
            pool,
            &tasker_db::CreateTask {
                queue_key: queue.to_string(),
                title: title.to_string(),
                brief: "Brief".to_string(),
                priority: "normal".to_string(),
                state: "ready".to_string(),
                review_required: false,
                acceptance_criteria: vec!["accepted".to_string()],
                validation_items: vec!["validated".to_string()],
                tags: vec![],
                conflict_hints: vec![],
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await?;
        Ok(())
    }

    async fn ids(pool: &SqlitePool, identifier: &str) -> (String, String) {
        sqlx::query_as::<_, (String, String)>(
            "SELECT id, task_queue_id FROM tasks WHERE identifier = ?",
        )
        .bind(identifier)
        .fetch_one(pool)
        .await
        .expect("ids")
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_run(
        pool: &SqlitePool,
        task_id: &str,
        queue_id: &str,
        run_id: &str,
        created_at: &str,
        finished_at: &str,
        outcome: Option<&str>,
        failure_reason: Option<&str>,
    ) {
        sqlx::query(
            r#"
            INSERT INTO agent_runs (
                id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                worker_actor_display_name, worker_id, launcher_kind, lease_expires_at,
                outcome, failure_reason, created_at, finished_at
            ) VALUES (?, ?, ?, 'worker_agent', 'worker', 'worker', 'worker', 'pi', ?, ?, ?, ?, ?)
            "#,
        )
        .bind(run_id)
        .bind(task_id)
        .bind(queue_id)
        .bind("2026-01-01 01:00:00")
        .bind(outcome)
        .bind(failure_reason)
        .bind(created_at)
        .bind(finished_at)
        .execute(pool)
        .await
        .expect("insert run");
    }

    async fn insert_integrating_event(pool: &SqlitePool, task_id: &str, created_at: &str) {
        sqlx::query(
            r#"
            INSERT INTO audit_events (
                id, actor_kind, actor_id, actor_display_name, event_type,
                subject_type, subject_id, payload_json, created_at
            ) VALUES ('integrating-event', 'operator', 'tester', 'tester',
                'task.state_transitioned', 'task', ?, '{"identifier":"TASK-1","from":"in_progress","to":"integrating"}', ?)
            "#,
        )
        .bind(task_id)
        .bind(created_at)
        .execute(pool)
        .await
        .expect("insert event");
    }
}

use anyhow::{Context, Result};
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifecycleTelemetryOptions {
    pub queue: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleTelemetrySummary {
    pub queues: Vec<QueueLifecycleTelemetry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueLifecycleTelemetry {
    pub queue_key: String,
    pub tasks: Vec<TaskLifecycleTelemetry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLifecycleTelemetry {
    pub identifier: String,
    pub title: String,
    pub state: String,
    pub ready_to_in_progress_seconds: Option<i64>,
    pub in_progress_to_integrating_seconds: Option<i64>,
    pub integrating_to_done_seconds: Option<i64>,
    pub ready_to_done_seconds: Option<i64>,
    pub missing_history: Vec<&'static str>,
}

#[derive(Debug, FromRow)]
struct TaskRow {
    id: String,
    queue_key: String,
    identifier: String,
    title: String,
    state: String,
}

#[derive(Debug, FromRow)]
struct AuditRow {
    subject_id: String,
    event_type: String,
    payload_json: String,
    created_epoch: i64,
}

#[derive(Debug, Clone, Copy, Default)]
struct StateTimes {
    ready: Option<i64>,
    in_progress: Option<i64>,
    integrating: Option<i64>,
    done: Option<i64>,
}

pub async fn lifecycle_summary(
    pool: &SqlitePool,
    options: &LifecycleTelemetryOptions,
) -> Result<LifecycleTelemetrySummary> {
    let limit = i64::try_from(options.limit.max(1)).unwrap_or(i64::MAX);
    let tasks = load_tasks(pool, options.queue.as_deref(), limit).await?;
    if tasks.is_empty() {
        return Ok(LifecycleTelemetrySummary { queues: Vec::new() });
    }

    let task_ids: Vec<_> = tasks.iter().map(|task| task.id.clone()).collect();
    let audits = load_lifecycle_audits(pool, &task_ids).await?;

    let mut queues: Vec<QueueLifecycleTelemetry> = Vec::new();
    for task in tasks {
        let times = derive_times(&task.id, &audits);
        let mut missing_history = Vec::new();
        if times.ready.is_none() {
            missing_history.push("ready");
        }
        if times.in_progress.is_none() {
            missing_history.push("in_progress");
        }
        if times.integrating.is_none() {
            missing_history.push("integrating");
        }
        if times.done.is_none() {
            missing_history.push("done");
        }

        let telemetry = TaskLifecycleTelemetry {
            identifier: task.identifier,
            title: task.title,
            state: task.state,
            ready_to_in_progress_seconds: duration_between(times.ready, times.in_progress),
            in_progress_to_integrating_seconds: duration_between(
                times.in_progress,
                times.integrating,
            ),
            integrating_to_done_seconds: duration_between(times.integrating, times.done),
            ready_to_done_seconds: duration_between(times.ready, times.done),
            missing_history,
        };

        if let Some(queue) = queues
            .iter_mut()
            .find(|queue| queue.queue_key == task.queue_key)
        {
            queue.tasks.push(telemetry);
        } else {
            queues.push(QueueLifecycleTelemetry {
                queue_key: task.queue_key,
                tasks: vec![telemetry],
            });
        }
    }

    for queue in &mut queues {
        queue.tasks.sort_by(|left, right| {
            slowest_score(right)
                .cmp(&slowest_score(left))
                .then_with(|| right.identifier.cmp(&left.identifier))
        });
    }

    Ok(LifecycleTelemetrySummary { queues })
}

pub fn render_lifecycle_summary(summary: &LifecycleTelemetrySummary) -> String {
    let mut output = String::new();
    output.push_str("Task lifecycle latency telemetry\n");
    if summary.queues.is_empty() {
        output.push_str("No Tasks found for lifecycle telemetry.\n");
        return output;
    }

    for queue in &summary.queues {
        output.push_str(&format!("\nTask Queue: {}\n", queue.queue_key));
        output.push_str("Recent slowest Tasks by total Ready → Done latency:\n");
        for task in &queue.tasks {
            output.push_str(&format!(
                "  {} — {} [{}]\n",
                task.identifier, task.title, task.state
            ));
            output.push_str(&format!(
                "    Ready → In Progress: {}; In Progress → Integrating: {}; Integrating → Done: {}; Ready → Done: {}\n",
                format_duration(task.ready_to_in_progress_seconds),
                format_duration(task.in_progress_to_integrating_seconds),
                format_duration(task.integrating_to_done_seconds),
                format_duration(task.ready_to_done_seconds),
            ));
            output.push_str(&format!("    bottleneck hint: {}\n", bottleneck_hint(task)));
            if !task.missing_history.is_empty() {
                output.push_str(&format!(
                    "    incomplete history: missing {} transition timestamp(s)\n",
                    task.missing_history.join(", ")
                ));
            }
        }
    }
    output
}

async fn load_tasks(pool: &SqlitePool, queue: Option<&str>, limit: i64) -> Result<Vec<TaskRow>> {
    sqlx::query_as::<_, TaskRow>(
        r#"
        SELECT tasks.id, task_queues.key AS queue_key, tasks.identifier, tasks.title, tasks.state
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE (? IS NULL OR task_queues.key = ?)
        ORDER BY
          COALESCE((
            SELECT MAX(unixepoch(audit_events.created_at))
            FROM audit_events
            WHERE audit_events.subject_type = 'task'
              AND audit_events.subject_id = tasks.id
              AND audit_events.event_type IN ('task.state_transitioned', 'task.state_changed')
              AND json_extract(audit_events.payload_json, '$.to') = 'done'
          ), unixepoch(tasks.updated_at)) DESC,
          tasks.identifier DESC
        LIMIT ?
        "#,
    )
    .bind(queue)
    .bind(queue)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("failed to load Tasks for lifecycle telemetry")
}

async fn load_lifecycle_audits(pool: &SqlitePool, task_ids: &[String]) -> Result<Vec<AuditRow>> {
    if task_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat("?")
        .take(task_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        r#"
        SELECT subject_id, event_type, payload_json, unixepoch(created_at) AS created_epoch
        FROM audit_events
        WHERE subject_type = 'task'
          AND event_type IN ('task.created', 'task.state_transitioned', 'task.state_changed')
          AND subject_id IN ({placeholders})
        ORDER BY created_at, id
        "#
    );
    let mut query = sqlx::query_as::<_, AuditRow>(&sql);
    for id in task_ids {
        query = query.bind(id);
    }
    query
        .fetch_all(pool)
        .await
        .context("failed to load Audit Events for lifecycle telemetry")
}

fn derive_times(task_id: &str, audits: &[AuditRow]) -> StateTimes {
    let mut times = StateTimes::default();
    for audit in audits.iter().filter(|audit| audit.subject_id == task_id) {
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(&audit.payload_json) else {
            continue;
        };
        match audit.event_type.as_str() {
            "task.created" => {
                if payload.get("state").and_then(|value| value.as_str()) == Some("ready") {
                    set_once(&mut times.ready, audit.created_epoch);
                }
            }
            "task.state_transitioned" | "task.state_changed" => {
                match payload.get("to").and_then(|value| value.as_str()) {
                    Some("ready") => set_once(&mut times.ready, audit.created_epoch),
                    Some("in_progress") => set_once(&mut times.in_progress, audit.created_epoch),
                    Some("integrating") => set_once(&mut times.integrating, audit.created_epoch),
                    Some("done") => set_once(&mut times.done, audit.created_epoch),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    times
}

fn set_once(slot: &mut Option<i64>, value: i64) {
    if slot.is_none() {
        *slot = Some(value);
    }
}

fn duration_between(start: Option<i64>, end: Option<i64>) -> Option<i64> {
    let duration = end? - start?;
    (duration >= 0).then_some(duration)
}

fn slowest_score(task: &TaskLifecycleTelemetry) -> i64 {
    task.ready_to_done_seconds
        .or(task.integrating_to_done_seconds)
        .or(task.in_progress_to_integrating_seconds)
        .or(task.ready_to_in_progress_seconds)
        .unwrap_or(-1)
}

fn format_duration(seconds: Option<i64>) -> String {
    match seconds {
        Some(seconds) if seconds < 60 => format!("{seconds}s"),
        Some(seconds) if seconds < 3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        Some(seconds) => format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60),
        None => "n/a".to_string(),
    }
}

fn bottleneck_hint(task: &TaskLifecycleTelemetry) -> &'static str {
    match (
        task.in_progress_to_integrating_seconds,
        task.integrating_to_done_seconds,
    ) {
        (Some(agent), Some(integrating)) if integrating > agent => {
            "Integrating or Manual Dogfood Merge wait dominates"
        }
        (Some(_), Some(_)) => "agent execution wait dominates",
        (Some(_), None) => {
            "agent execution duration available; Integrating/Done history incomplete"
        }
        (None, Some(_)) => "Integrating duration available; agent execution history incomplete",
        (None, None) => "not enough lifecycle history to identify bottleneck",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

    async fn memory_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrations");
        pool
    }

    async fn seed_queue_and_task(
        pool: &SqlitePool,
        identifier: &str,
        title: &str,
        state: &str,
    ) -> String {
        let queue_id = "queue-1";
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO task_queues (
                id, key, name, managed_source_repository, main_branch, worktree_root, branch_template
            ) VALUES (?, 'TASK', 'Tasker', '/repo', 'main', '/worktrees', 'tasker/{task_identifier}')
            "#,
        )
        .bind(queue_id)
        .execute(pool)
        .await
        .expect("queue");
        let task_id = identifier.to_string();
        let sequence = identifier
            .rsplit_once('-')
            .and_then(|(_, sequence)| sequence.parse::<i64>().ok())
            .unwrap_or(1);
        sqlx::query(
            r#"
            INSERT INTO tasks (id, task_queue_id, identifier, sequence, title, brief, priority, state)
            VALUES (?, ?, ?, ?, ?, 'brief', 'normal', ?)
            "#,
        )
        .bind(&task_id)
        .bind(queue_id)
        .bind(identifier)
        .bind(sequence)
        .bind(title)
        .bind(state)
        .execute(pool)
        .await
        .expect("task");
        task_id
    }

    async fn audit(
        pool: &SqlitePool,
        task_id: &str,
        event_type: &str,
        payload: serde_json::Value,
        at: &str,
    ) {
        sqlx::query(
            r#"
            INSERT INTO audit_events (
                id, actor_kind, actor_id, actor_display_name, event_type, subject_type, subject_id, payload_json, created_at
            ) VALUES (?, 'operator', 'tester', 'tester', ?, 'task', ?, ?, ?)
            "#,
        )
        .bind(format!("event-{task_id}-{event_type}-{at}"))
        .bind(event_type)
        .bind(task_id)
        .bind(payload.to_string())
        .bind(at)
        .execute(pool)
        .await
        .expect("audit");
    }

    #[tokio::test]
    async fn telemetry_lifecycle_derives_durations_from_audit_events() {
        let pool = memory_pool().await;
        let task_id = seed_queue_and_task(&pool, "TASK-1", "Finished task", "done").await;
        audit(
            &pool,
            &task_id,
            "task.created",
            serde_json::json!({"state":"ready"}),
            "2026-01-01 00:00:00",
        )
        .await;
        audit(
            &pool,
            &task_id,
            "task.state_changed",
            serde_json::json!({"from":"ready","to":"in_progress"}),
            "2026-01-01 00:05:00",
        )
        .await;
        audit(
            &pool,
            &task_id,
            "task.state_transitioned",
            serde_json::json!({"from":"in_progress","to":"integrating"}),
            "2026-01-01 01:00:00",
        )
        .await;
        audit(
            &pool,
            &task_id,
            "task.state_transitioned",
            serde_json::json!({"from":"integrating","to":"done"}),
            "2026-01-01 03:00:00",
        )
        .await;

        let summary = lifecycle_summary(
            &pool,
            &LifecycleTelemetryOptions {
                queue: Some("TASK".to_string()),
                limit: 5,
            },
        )
        .await
        .expect("summary");
        let task = &summary.queues[0].tasks[0];

        assert_eq!(task.ready_to_in_progress_seconds, Some(300));
        assert_eq!(task.in_progress_to_integrating_seconds, Some(3300));
        assert_eq!(task.integrating_to_done_seconds, Some(7200));
        assert_eq!(task.ready_to_done_seconds, Some(10800));
        assert!(render_lifecycle_summary(&summary)
            .contains("Integrating or Manual Dogfood Merge wait dominates"));
    }

    #[tokio::test]
    async fn telemetry_lifecycle_handles_incomplete_and_missing_transition_histories() {
        let pool = memory_pool().await;
        let first = seed_queue_and_task(&pool, "TASK-1", "Incomplete task", "integrating").await;
        audit(
            &pool,
            &first,
            "task.created",
            serde_json::json!({"state":"ready"}),
            "2026-01-01 00:00:00",
        )
        .await;
        audit(
            &pool,
            &first,
            "task.state_changed",
            serde_json::json!({"from":"ready","to":"in_progress"}),
            "2026-01-01 00:01:00",
        )
        .await;
        let _second = seed_queue_and_task(&pool, "TASK-2", "Missing history task", "done").await;

        let summary = lifecycle_summary(
            &pool,
            &LifecycleTelemetryOptions {
                queue: Some("TASK".to_string()),
                limit: 5,
            },
        )
        .await
        .expect("summary");
        let rendered = render_lifecycle_summary(&summary);

        assert!(rendered.contains("Incomplete task"));
        assert!(rendered.contains("Missing history task"));
        assert!(rendered.contains("incomplete history"));
        assert!(summary.queues[0]
            .tasks
            .iter()
            .any(|task| task.identifier == "TASK-2" && task.ready_to_done_seconds.is_none()));
    }
}

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};
use std::{collections::BTreeMap, fmt::Write};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryOptions {
    pub queue: String,
    pub slow_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TelemetrySummary {
    pub queue: String,
    pub total_runs: usize,
    pub duplicate_tasks: Vec<DuplicateTaskRun>,
    pub post_integrating_runs: Vec<TelemetryRun>,
    pub failed_or_timed_out_runs: Vec<TelemetryRun>,
    pub completed_duration_seconds: Vec<i64>,
    pub slowest_completed_runs: Vec<TelemetryRun>,
    pub efficiency: EfficiencyTelemetrySummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DuplicateTaskRun {
    pub task_identifier: String,
    pub task_title: String,
    pub run_count: usize,
    pub wasted_runs: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TelemetryRun {
    pub task_identifier: String,
    pub task_title: String,
    pub agent_run_id: String,
    pub outcome: Option<String>,
    pub failure_reason: Option<String>,
    pub failure_reason_code: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub duration_seconds: Option<i64>,
    pub tool_call_count: Option<i64>,
    pub tool_error_count: Option<i64>,
    pub repeated_failed_tool_attempt_count: Option<i64>,
    pub tool_call_counts: BTreeMap<String, i64>,
    pub repeated_read_count: Option<i64>,
    pub repeated_tasker_context_fetch_count: Option<i64>,
    pub shell_command_counts: BTreeMap<String, i64>,
    pub assistant_turn_count: Option<i64>,
    pub user_turn_count: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub max_context_tokens: Option<i64>,
    pub efficiency_hints_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EfficiencyTelemetrySummary {
    pub runs_with_metrics: usize,
    pub total_tool_calls: i64,
    pub total_tool_errors: i64,
    pub total_repeated_failed_tool_attempts: i64,
    pub tool_call_counts: BTreeMap<String, i64>,
    pub total_repeated_reads: i64,
    pub total_repeated_tasker_context_fetches: i64,
    pub shell_command_counts: BTreeMap<String, i64>,
    pub total_assistant_turns: i64,
    pub total_user_turns: i64,
    pub max_input_tokens: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub max_total_tokens: Option<i64>,
    pub max_cache_read_tokens: Option<i64>,
    pub max_cache_write_tokens: Option<i64>,
    pub max_context_tokens: Option<i64>,
    pub inefficient_runs: Vec<TelemetryRun>,
}

#[derive(Debug, FromRow)]
struct TelemetryRunRow {
    task_identifier: String,
    task_title: String,
    agent_run_id: String,
    outcome: Option<String>,
    failure_reason: Option<String>,
    failure_reason_code: Option<String>,
    created_at: String,
    finished_at: Option<String>,
    integrating_at: Option<String>,
    duration_seconds: Option<i64>,
    tool_call_count: Option<i64>,
    tool_error_count: Option<i64>,
    repeated_failed_tool_attempt_count: Option<i64>,
    tool_call_counts_json: Option<String>,
    repeated_read_count: Option<i64>,
    repeated_tasker_context_fetch_count: Option<i64>,
    shell_command_counts_json: Option<String>,
    assistant_turn_count: Option<i64>,
    user_turn_count: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    total_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    max_context_tokens: Option<i64>,
    efficiency_hints_json: Option<String>,
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
            agent_runs.failure_reason_code AS failure_reason_code,
            agent_runs.created_at AS created_at,
            agent_runs.finished_at AS finished_at,
            first_integrating.integrating_at AS integrating_at,
            CASE
                WHEN COALESCE(launcher_session_data.finished_at, agent_runs.finished_at) IS NOT NULL
                 AND COALESCE(launcher_session_data.started_at, agent_runs.created_at) IS NOT NULL
                THEN CAST(strftime('%s', COALESCE(launcher_session_data.finished_at, agent_runs.finished_at)) AS INTEGER)
                   - CAST(strftime('%s', COALESCE(launcher_session_data.started_at, agent_runs.created_at)) AS INTEGER)
                ELSE NULL
            END AS duration_seconds,
            agent_run_metrics.tool_call_count AS tool_call_count,
            agent_run_metrics.tool_error_count AS tool_error_count,
            agent_run_metrics.repeated_failed_tool_attempt_count AS repeated_failed_tool_attempt_count,
            agent_run_metrics.tool_call_counts_json AS tool_call_counts_json,
            agent_run_metrics.repeated_read_count AS repeated_read_count,
            agent_run_metrics.repeated_tasker_context_fetch_count AS repeated_tasker_context_fetch_count,
            agent_run_metrics.shell_command_counts_json AS shell_command_counts_json,
            agent_run_metrics.assistant_turn_count AS assistant_turn_count,
            agent_run_metrics.user_turn_count AS user_turn_count,
            agent_run_metrics.input_tokens AS input_tokens,
            agent_run_metrics.output_tokens AS output_tokens,
            agent_run_metrics.total_tokens AS total_tokens,
            agent_run_metrics.cache_read_tokens AS cache_read_tokens,
            agent_run_metrics.cache_write_tokens AS cache_write_tokens,
            agent_run_metrics.max_context_tokens AS max_context_tokens,
            agent_run_metrics.efficiency_hints_json AS efficiency_hints_json
        FROM agent_runs
        JOIN tasks ON tasks.id = agent_runs.task_id
        JOIN task_queues ON task_queues.id = agent_runs.task_queue_id
        LEFT JOIN launcher_session_data ON launcher_session_data.agent_run_id = agent_runs.id
        LEFT JOIN agent_run_metrics ON agent_run_metrics.agent_run_id = agent_runs.id
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

    let efficiency = build_efficiency_summary(&rows);

    Ok(TelemetrySummary {
        queue: options.queue.clone(),
        total_runs: rows.len(),
        duplicate_tasks,
        post_integrating_runs,
        failed_or_timed_out_runs,
        completed_duration_seconds,
        slowest_completed_runs,
        efficiency,
    })
}

fn run_from_row(row: &TelemetryRunRow) -> TelemetryRun {
    TelemetryRun {
        task_identifier: row.task_identifier.clone(),
        task_title: row.task_title.clone(),
        agent_run_id: row.agent_run_id.clone(),
        outcome: row.outcome.clone(),
        failure_reason: row.failure_reason.clone(),
        failure_reason_code: row.failure_reason_code.clone(),
        created_at: row.created_at.clone(),
        finished_at: row.finished_at.clone(),
        duration_seconds: row.duration_seconds,
        tool_call_count: row.tool_call_count,
        tool_error_count: row.tool_error_count,
        repeated_failed_tool_attempt_count: row.repeated_failed_tool_attempt_count,
        tool_call_counts: json_counts(row.tool_call_counts_json.as_deref()),
        repeated_read_count: row.repeated_read_count,
        repeated_tasker_context_fetch_count: row.repeated_tasker_context_fetch_count,
        shell_command_counts: json_counts(row.shell_command_counts_json.as_deref()),
        assistant_turn_count: row.assistant_turn_count,
        user_turn_count: row.user_turn_count,
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        total_tokens: row.total_tokens,
        cache_read_tokens: row.cache_read_tokens,
        cache_write_tokens: row.cache_write_tokens,
        max_context_tokens: row.max_context_tokens,
        efficiency_hints_json: row.efficiency_hints_json.clone(),
    }
}

fn json_counts(json: Option<&str>) -> BTreeMap<String, i64> {
    json.and_then(|json| serde_json::from_str::<BTreeMap<String, i64>>(json).ok())
        .unwrap_or_default()
}

fn merge_counts(target: &mut BTreeMap<String, i64>, source: BTreeMap<String, i64>) {
    for (key, count) in source {
        *target.entry(key).or_insert(0) += count;
    }
}

fn render_counts(counts: &BTreeMap<String, i64>) -> String {
    if counts.is_empty() {
        "none".to_string()
    } else {
        counts
            .iter()
            .map(|(key, count)| format!("{key}={count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn build_efficiency_summary(rows: &[TelemetryRunRow]) -> EfficiencyTelemetrySummary {
    let mut summary = EfficiencyTelemetrySummary {
        runs_with_metrics: 0,
        total_tool_calls: 0,
        total_tool_errors: 0,
        total_repeated_failed_tool_attempts: 0,
        tool_call_counts: BTreeMap::new(),
        total_repeated_reads: 0,
        total_repeated_tasker_context_fetches: 0,
        shell_command_counts: BTreeMap::new(),
        total_assistant_turns: 0,
        total_user_turns: 0,
        max_input_tokens: None,
        max_output_tokens: None,
        max_total_tokens: None,
        max_cache_read_tokens: None,
        max_cache_write_tokens: None,
        max_context_tokens: None,
        inefficient_runs: Vec::new(),
    };
    for row in rows {
        let has_metrics = row.tool_call_count.is_some()
            || row.tool_error_count.is_some()
            || row.assistant_turn_count.is_some()
            || row.user_turn_count.is_some()
            || row.input_tokens.is_some()
            || row.output_tokens.is_some()
            || row.total_tokens.is_some()
            || row.cache_read_tokens.is_some()
            || row.cache_write_tokens.is_some()
            || row.max_context_tokens.is_some()
            || row.repeated_read_count.is_some()
            || row.repeated_tasker_context_fetch_count.is_some()
            || row.tool_call_counts_json.is_some()
            || row.shell_command_counts_json.is_some();
        if has_metrics {
            summary.runs_with_metrics += 1;
        }
        summary.total_tool_calls += row.tool_call_count.unwrap_or(0);
        summary.total_tool_errors += row.tool_error_count.unwrap_or(0);
        summary.total_repeated_failed_tool_attempts +=
            row.repeated_failed_tool_attempt_count.unwrap_or(0);
        merge_counts(
            &mut summary.tool_call_counts,
            json_counts(row.tool_call_counts_json.as_deref()),
        );
        summary.total_repeated_reads += row.repeated_read_count.unwrap_or(0);
        summary.total_repeated_tasker_context_fetches +=
            row.repeated_tasker_context_fetch_count.unwrap_or(0);
        merge_counts(
            &mut summary.shell_command_counts,
            json_counts(row.shell_command_counts_json.as_deref()),
        );
        summary.total_assistant_turns += row.assistant_turn_count.unwrap_or(0);
        summary.total_user_turns += row.user_turn_count.unwrap_or(0);
        if let Some(tokens) = row.input_tokens {
            summary.max_input_tokens = Some(summary.max_input_tokens.unwrap_or(0).max(tokens));
        }
        if let Some(tokens) = row.output_tokens {
            summary.max_output_tokens = Some(summary.max_output_tokens.unwrap_or(0).max(tokens));
        }
        if let Some(tokens) = row.total_tokens {
            summary.max_total_tokens = Some(summary.max_total_tokens.unwrap_or(0).max(tokens));
        }
        if let Some(tokens) = row.cache_read_tokens {
            summary.max_cache_read_tokens =
                Some(summary.max_cache_read_tokens.unwrap_or(0).max(tokens));
        }
        if let Some(tokens) = row.cache_write_tokens {
            summary.max_cache_write_tokens =
                Some(summary.max_cache_write_tokens.unwrap_or(0).max(tokens));
        }
        if let Some(tokens) = row.max_context_tokens {
            summary.max_context_tokens = Some(summary.max_context_tokens.unwrap_or(0).max(tokens));
        }
        let hints = row
            .efficiency_hints_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok())
            .unwrap_or_default();
        if !hints.is_empty()
            || row.tool_call_count.unwrap_or(0) >= 30
            || row.repeated_failed_tool_attempt_count.unwrap_or(0) > 0
            || row.repeated_read_count.unwrap_or(0) > 0
            || row.repeated_tasker_context_fetch_count.unwrap_or(0) > 0
        {
            summary.inefficient_runs.push(run_from_row(row));
        }
    }
    summary.inefficient_runs.sort_by(|left, right| {
        right
            .tool_call_count
            .unwrap_or(0)
            .cmp(&left.tool_call_count.unwrap_or(0))
            .then_with(|| left.task_identifier.cmp(&right.task_identifier))
    });
    summary
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

    writeln!(
        output,
        "efficiency metrics: {} run(s), {} tool call(s), {} tool error(s), {} repeated failed tool attempt(s), {} repeated read(s), {} repeated Tasker context fetch(es), assistant/user turns {}/{}, max tokens input/output/total {}/{}/{}, cache read/write {}/{}, max context tokens {}",
        summary.efficiency.runs_with_metrics,
        summary.efficiency.total_tool_calls,
        summary.efficiency.total_tool_errors,
        summary.efficiency.total_repeated_failed_tool_attempts,
        summary.efficiency.total_repeated_reads,
        summary.efficiency.total_repeated_tasker_context_fetches,
        summary.efficiency.total_assistant_turns,
        summary.efficiency.total_user_turns,
        summary.efficiency.max_input_tokens.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string()),
        summary.efficiency.max_output_tokens.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string()),
        summary.efficiency.max_total_tokens.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string()),
        summary.efficiency.max_cache_read_tokens.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string()),
        summary.efficiency.max_cache_write_tokens.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string()),
        summary.efficiency.max_context_tokens.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string())
    )
    .expect("write string");
    writeln!(
        output,
        "  tool calls by tool: {}",
        render_counts(&summary.efficiency.tool_call_counts)
    )
    .expect("write string");
    writeln!(
        output,
        "  shell command categories: {}",
        render_counts(&summary.efficiency.shell_command_counts)
    )
    .expect("write string");
    if !summary.efficiency.inefficient_runs.is_empty() {
        writeln!(output, "optimization hints:").expect("write string");
        for run in &summary.efficiency.inefficient_runs {
            let hints = run.efficiency_hints_json.as_deref().unwrap_or("[]");
            writeln!(
                output,
                "  {} - {}: {} tool_calls={} tool_errors={} repeated_reads={} repeated_tasker_context_fetches={} hints={}",
                run.task_identifier,
                run.task_title,
                run.agent_run_id,
                run.tool_call_count.unwrap_or(0),
                run.tool_error_count.unwrap_or(0),
                run.repeated_read_count.unwrap_or(0),
                run.repeated_tasker_context_fetch_count.unwrap_or(0),
                hints
            )
            .expect("write string");
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrelationOptions {
    pub queue: String,
    pub landing_tasks: Vec<String>,
    pub landing_commits: Vec<String>,
    pub landing_timestamps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CorrelationSummary {
    pub queue: String,
    pub landing_points: Vec<LandingPointSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LandingPointSummary {
    pub label: String,
    pub source: String,
    pub landed_at: String,
    pub before: CorrelationBucket,
    pub after: CorrelationBucket,
    pub interpretations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CorrelationBucket {
    pub agent_runs: usize,
    pub duplicate_agent_run_waste: usize,
    pub duplicate_tasks: usize,
    pub post_integrating_agent_runs: usize,
    pub failed_agent_runs: usize,
    pub failed_agent_runs_by_reason: Vec<FailureReasonCount>,
    pub completed_run_count: usize,
    pub average_completed_run_duration_seconds: Option<i64>,
    pub integrating_wait_count: usize,
    pub average_integrating_wait_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailureReasonCount {
    pub reason: String,
    pub count: usize,
}

#[derive(Debug, FromRow)]
struct LandingPointRow {
    label: String,
    source: String,
    landed_at: String,
    landed_epoch: i64,
}

#[derive(Debug, Clone, FromRow)]
struct CorrelationRunRow {
    task_identifier: String,
    outcome: Option<String>,
    failure_reason: Option<String>,
    failure_reason_code: Option<String>,
    created_epoch: i64,
    integrating_epoch: Option<i64>,
    duration_seconds: Option<i64>,
}

#[derive(Debug, Clone, FromRow)]
struct IntegratingWaitRow {
    integrating_epoch: i64,
    wait_seconds: i64,
}

pub async fn correlation_summary(
    pool: &SqlitePool,
    options: &CorrelationOptions,
) -> Result<CorrelationSummary> {
    let landing_points = resolve_landing_points(pool, options).await?;
    let runs = load_correlation_runs(pool, &options.queue).await?;
    let waits = load_integrating_waits(pool, &options.queue).await?;
    let landing_points = landing_points
        .into_iter()
        .map(|landing| {
            let before = build_correlation_bucket(
                runs.iter()
                    .filter(|run| run.created_epoch < landing.landed_epoch),
                waits
                    .iter()
                    .filter(|wait| wait.integrating_epoch < landing.landed_epoch),
            );
            let after = build_correlation_bucket(
                runs.iter()
                    .filter(|run| run.created_epoch >= landing.landed_epoch),
                waits
                    .iter()
                    .filter(|wait| wait.integrating_epoch >= landing.landed_epoch),
            );
            let interpretations = interpret_correlation(&before, &after);
            LandingPointSummary {
                label: landing.label,
                source: landing.source,
                landed_at: landing.landed_at,
                before,
                after,
                interpretations,
            }
        })
        .collect();
    Ok(CorrelationSummary {
        queue: options.queue.clone(),
        landing_points,
    })
}

async fn resolve_landing_points(
    pool: &SqlitePool,
    options: &CorrelationOptions,
) -> Result<Vec<LandingPointRow>> {
    let mut points = Vec::new();
    for identifier in &options.landing_tasks {
        let point = sqlx::query_as::<_, LandingPointRow>(
            r#"
            SELECT tasks.identifier AS label, 'task' AS source, COALESCE(done_events.done_at, tasks.updated_at) AS landed_at,
                   unixepoch(COALESCE(done_events.done_at, tasks.updated_at)) AS landed_epoch
            FROM tasks
            JOIN task_queues ON task_queues.id = tasks.task_queue_id
            LEFT JOIN (
                SELECT subject_id, MIN(created_at) AS done_at
                FROM audit_events
                WHERE subject_type = 'task'
                  AND event_type IN ('task.state_transitioned', 'task.state_changed')
                  AND json_extract(payload_json, '$.to') = 'done'
                GROUP BY subject_id
            ) done_events ON done_events.subject_id = tasks.id
            WHERE task_queues.key = ? AND tasks.identifier = ?
            "#,
        )
        .bind(&options.queue)
        .bind(identifier)
        .fetch_one(pool)
        .await
        .with_context(|| format!("failed to resolve landing Task {identifier}"))?;
        points.push(point);
    }
    for commit in &options.landing_commits {
        let point = sqlx::query_as::<_, LandingPointRow>(
            r#"
            SELECT integration_outcomes.final_commit AS label, 'commit' AS source, integration_outcomes.created_at AS landed_at,
                   unixepoch(integration_outcomes.created_at) AS landed_epoch
            FROM integration_outcomes
            JOIN tasks ON tasks.id = integration_outcomes.task_id
            JOIN task_queues ON task_queues.id = tasks.task_queue_id
            WHERE task_queues.key = ? AND integration_outcomes.final_commit = ?
            ORDER BY integration_outcomes.created_at
            LIMIT 1
            "#,
        )
        .bind(&options.queue)
        .bind(commit)
        .fetch_one(pool)
        .await
        .with_context(|| format!("failed to resolve landing commit {commit}"))?;
        points.push(point);
    }
    for timestamp in &options.landing_timestamps {
        let landed_epoch = sqlx::query_scalar::<_, i64>("SELECT unixepoch(?)")
            .bind(timestamp)
            .fetch_one(pool)
            .await
            .with_context(|| format!("failed to parse landing timestamp {timestamp}"))?;
        points.push(LandingPointRow {
            label: timestamp.clone(),
            source: "timestamp".to_string(),
            landed_at: timestamp.clone(),
            landed_epoch,
        });
    }
    points.sort_by(|left, right| left.landed_at.cmp(&right.landed_at));
    Ok(points)
}

async fn load_correlation_runs(pool: &SqlitePool, queue: &str) -> Result<Vec<CorrelationRunRow>> {
    sqlx::query_as::<_, CorrelationRunRow>(
        r#"
        WITH first_integrating AS (
            SELECT subject_id AS task_id, MIN(unixepoch(created_at)) AS integrating_epoch
            FROM audit_events
            WHERE subject_type = 'task'
              AND event_type IN ('task.state_transitioned', 'task.state_changed')
              AND json_extract(payload_json, '$.to') = 'integrating'
            GROUP BY subject_id
        )
        SELECT
            tasks.identifier AS task_identifier,
            agent_runs.outcome AS outcome,
            agent_runs.failure_reason AS failure_reason,
            agent_runs.failure_reason_code AS failure_reason_code,
            unixepoch(agent_runs.created_at) AS created_epoch,
            first_integrating.integrating_epoch AS integrating_epoch,
            CASE
                WHEN COALESCE(launcher_session_data.finished_at, agent_runs.finished_at) IS NOT NULL
                 AND COALESCE(launcher_session_data.started_at, agent_runs.created_at) IS NOT NULL
                THEN unixepoch(COALESCE(launcher_session_data.finished_at, agent_runs.finished_at))
                   - unixepoch(COALESCE(launcher_session_data.started_at, agent_runs.created_at))
                ELSE NULL
            END AS duration_seconds
        FROM agent_runs
        JOIN tasks ON tasks.id = agent_runs.task_id
        JOIN task_queues ON task_queues.id = agent_runs.task_queue_id
        LEFT JOIN launcher_session_data ON launcher_session_data.agent_run_id = agent_runs.id
        LEFT JOIN first_integrating ON first_integrating.task_id = tasks.id
        WHERE task_queues.key = ?
        ORDER BY agent_runs.created_at, agent_runs.id
        "#,
    )
    .bind(queue)
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to load correlation Agent Runs for Task Queue {queue}"))
}

async fn load_integrating_waits(pool: &SqlitePool, queue: &str) -> Result<Vec<IntegratingWaitRow>> {
    sqlx::query_as::<_, IntegratingWaitRow>(
        r#"
        WITH integrating AS (
            SELECT subject_id AS task_id, MIN(unixepoch(created_at)) AS integrating_epoch
            FROM audit_events
            WHERE subject_type = 'task'
              AND event_type IN ('task.state_transitioned', 'task.state_changed')
              AND json_extract(payload_json, '$.to') = 'integrating'
            GROUP BY subject_id
        ), done AS (
            SELECT subject_id AS task_id, MIN(unixepoch(created_at)) AS done_epoch
            FROM audit_events
            WHERE subject_type = 'task'
              AND event_type IN ('task.state_transitioned', 'task.state_changed')
              AND json_extract(payload_json, '$.to') = 'done'
            GROUP BY subject_id
        )
        SELECT integrating.integrating_epoch AS integrating_epoch,
               done.done_epoch - integrating.integrating_epoch AS wait_seconds
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        JOIN integrating ON integrating.task_id = tasks.id
        JOIN done ON done.task_id = tasks.id
        WHERE task_queues.key = ? AND done.done_epoch >= integrating.integrating_epoch
        "#,
    )
    .bind(queue)
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to load Integrating wait telemetry for Task Queue {queue}"))
}

fn build_correlation_bucket<'a>(
    runs: impl Iterator<Item = &'a CorrelationRunRow>,
    waits: impl Iterator<Item = &'a IntegratingWaitRow>,
) -> CorrelationBucket {
    let runs: Vec<_> = runs.collect();
    let waits: Vec<_> = waits.collect();
    let mut task_counts = std::collections::BTreeMap::<&str, usize>::new();
    let mut failures = std::collections::BTreeMap::<String, usize>::new();
    let mut completed_durations = Vec::new();
    for run in &runs {
        *task_counts.entry(&run.task_identifier).or_default() += 1;
        if run.outcome.as_deref() == Some("failed") || run.outcome.as_deref() == Some("expired") {
            let reason = run
                .failure_reason_code
                .clone()
                .filter(|code| !code.trim().is_empty())
                .or_else(|| {
                    run.failure_reason
                        .clone()
                        .filter(|reason| !reason.trim().is_empty())
                })
                .unwrap_or_else(|| run.outcome.clone().unwrap_or_else(|| "unknown".to_string()));
            *failures.entry(reason).or_default() += 1;
        }
        if run.outcome.as_deref() == Some("completed") {
            if let Some(duration) = run.duration_seconds {
                completed_durations.push(duration);
            }
        }
    }
    let duplicate_agent_run_waste = task_counts
        .values()
        .map(|count| count.saturating_sub(1))
        .sum::<usize>();
    let failed_agent_runs = failures.values().sum();
    let integrating_waits = waits
        .iter()
        .map(|wait| wait.wait_seconds)
        .collect::<Vec<_>>();
    CorrelationBucket {
        agent_runs: runs.len(),
        duplicate_agent_run_waste,
        duplicate_tasks: task_counts.values().filter(|count| **count > 1).count(),
        post_integrating_agent_runs: runs
            .iter()
            .filter(|run| {
                run.integrating_epoch
                    .is_some_and(|integrating| run.created_epoch > integrating)
            })
            .count(),
        failed_agent_runs,
        failed_agent_runs_by_reason: failures
            .into_iter()
            .map(|(reason, count)| FailureReasonCount { reason, count })
            .collect(),
        completed_run_count: completed_durations.len(),
        average_completed_run_duration_seconds: average_i64(&completed_durations),
        integrating_wait_count: integrating_waits.len(),
        average_integrating_wait_seconds: average_i64(&integrating_waits),
    }
}

fn average_i64(values: &[i64]) -> Option<i64> {
    (!values.is_empty())
        .then(|| values.iter().sum::<i64>() / i64::try_from(values.len()).unwrap_or(1))
}

fn interpret_correlation(before: &CorrelationBucket, after: &CorrelationBucket) -> Vec<String> {
    vec![
        interpret_count(
            "duplicate Agent Run waste",
            before.duplicate_agent_run_waste,
            after.duplicate_agent_run_waste,
        ),
        interpret_count(
            "post-Integrating Agent Runs",
            before.post_integrating_agent_runs,
            after.post_integrating_agent_runs,
        ),
        interpret_count(
            "failed Agent Runs",
            before.failed_agent_runs,
            after.failed_agent_runs,
        ),
        interpret_optional_average(
            "completed run duration",
            before.average_completed_run_duration_seconds,
            after.average_completed_run_duration_seconds,
        ),
        interpret_optional_average(
            "Integrating wait time",
            before.average_integrating_wait_seconds,
            after.average_integrating_wait_seconds,
        ),
    ]
}

fn interpret_count(label: &str, before: usize, after: usize) -> String {
    match (before, after) {
        (0, 0) => format!("{label}: no observed problem before or after this landing point"),
        (_, 0) => format!("{label}: appears historical after this landing point"),
        (0, _) => format!("{label}: appears newly active after this landing point"),
        (before, after) if after < before => format!("{label}: improved after this landing point"),
        (before, after) if after > before => {
            format!("{label}: still active and worse after this landing point")
        }
        _ => format!("{label}: still active at about the same level after this landing point"),
    }
}

fn interpret_optional_average(label: &str, before: Option<i64>, after: Option<i64>) -> String {
    match (before, after) {
        (None, None) => format!("{label}: no comparable data"),
        (Some(_), None) => format!("{label}: appears historical after this landing point"),
        (None, Some(_)) => format!("{label}: now has active observations after this landing point"),
        (Some(before), Some(after)) if after < before => {
            format!("{label}: improved after this landing point")
        }
        (Some(before), Some(after)) if after > before => {
            format!("{label}: still active and slower after this landing point")
        }
        _ => format!("{label}: still active at about the same level after this landing point"),
    }
}

pub fn render_correlation_summary(summary: &CorrelationSummary) -> String {
    let mut output = String::new();
    writeln!(output, "Telemetry correlation summary").expect("write string");
    writeln!(output, "Task Queue: {}", summary.queue).expect("write string");
    if summary.landing_points.is_empty() {
        writeln!(
            output,
            "No landing points supplied. Use --landing-task, --landing-commit, or --landing-at."
        )
        .expect("write string");
        return output;
    }
    for landing in &summary.landing_points {
        writeln!(
            output,
            "\nLanding point: {} ({}) at {}",
            landing.label, landing.source, landing.landed_at
        )
        .expect("write string");
        write_bucket(&mut output, "before", &landing.before);
        write_bucket(&mut output, "after", &landing.after);
        writeln!(output, "interpretation guidance:").expect("write string");
        for interpretation in &landing.interpretations {
            writeln!(output, "  - {interpretation}").expect("write string");
        }
        writeln!(
            output,
            "  - Focus current work on metrics that remain active after this landing point; treat before-only counts as cleanup noise unless they still block delivery."
        )
        .expect("write string");
    }
    output
}

fn write_bucket(output: &mut String, label: &str, bucket: &CorrelationBucket) {
    writeln!(output, "  {label}:").expect("write string");
    writeln!(output, "    Agent Runs: {}", bucket.agent_runs).expect("write string");
    writeln!(
        output,
        "    duplicate Agent Run waste: {} wasted across {} Task(s)",
        bucket.duplicate_agent_run_waste, bucket.duplicate_tasks
    )
    .expect("write string");
    writeln!(
        output,
        "    post-Integrating Agent Runs: {}",
        bucket.post_integrating_agent_runs
    )
    .expect("write string");
    writeln!(
        output,
        "    failed Agent Runs: {}",
        bucket.failed_agent_runs
    )
    .expect("write string");
    for failure in &bucket.failed_agent_runs_by_reason {
        writeln!(output, "      {}: {}", failure.reason, failure.count).expect("write string");
    }
    writeln!(
        output,
        "    completed run duration avg: {}",
        format_duration(bucket.average_completed_run_duration_seconds)
    )
    .expect("write string");
    writeln!(
        output,
        "    Integrating wait avg: {} across {} Task(s)",
        format_duration(bucket.average_integrating_wait_seconds),
        bucket.integrating_wait_count
    )
    .expect("write string");
}

// Task lifecycle latency telemetry
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
    let placeholders = std::iter::repeat_n("?", task_ids.len())
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
            "task.created"
                if payload.get("state").and_then(|value| value.as_str()) == Some("ready") =>
            {
                set_once(&mut times.ready, audit.created_epoch);
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
        sqlx::query(
            r#"
            INSERT INTO agent_run_metrics (
                agent_run_id, launcher_kind, final_status, tool_call_count, tool_error_count,
                repeated_failed_tool_attempt_count, tool_call_counts_json, repeated_read_count,
                repeated_tasker_context_fetch_count, shell_command_counts_json,
                assistant_turn_count, user_turn_count, max_context_tokens, efficiency_hints_json
            ) VALUES ('run-2', 'pi', 'completed', 42, 6, 2,
                '{"read":13,"bash":25,"edit":4}', 3, 2,
                '{"tasker_cli":7,"cargo":3,"git":2,"search":8,"other":5}',
                9, 4, 123456,
                '["excessive tool calls","repeated failed tool attempts","repeated file reads","repeated Tasker context fetches","large context growth","validation/tool loop"]')
            "#,
        )
        .execute(&pool)
        .await
        .expect("metrics");

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
        assert_eq!(summary.efficiency.runs_with_metrics, 1);
        assert_eq!(summary.efficiency.total_tool_calls, 42);
        assert_eq!(summary.efficiency.total_tool_errors, 6);
        assert_eq!(summary.efficiency.total_repeated_failed_tool_attempts, 2);
        assert_eq!(summary.efficiency.tool_call_counts.get("read"), Some(&13));
        assert_eq!(summary.efficiency.tool_call_counts.get("bash"), Some(&25));
        assert_eq!(summary.efficiency.total_repeated_reads, 3);
        assert_eq!(summary.efficiency.total_repeated_tasker_context_fetches, 2);
        assert_eq!(
            summary.efficiency.shell_command_counts.get("tasker_cli"),
            Some(&7)
        );
        assert_eq!(summary.efficiency.max_context_tokens, Some(123456));
        assert_eq!(summary.efficiency.inefficient_runs[0].agent_run_id, "run-2");
        let json = serde_json::to_value(&summary).expect("json");
        assert_eq!(json["efficiency"]["total_tool_calls"], 42);
        assert_eq!(json["efficiency"]["tool_call_counts"]["bash"], 25);
        assert_eq!(json["efficiency"]["total_repeated_reads"], 3);
        assert_eq!(json["efficiency"]["shell_command_counts"]["search"], 8);
        assert!(json.to_string().contains("excessive tool calls"));
        assert!(!json.to_string().contains("brief"));
        let rendered = render_summary(&summary);
        assert!(rendered.contains("TASK-1 - Repeated work"));
        assert!(rendered.contains("post-Integrating Agent Runs: 1"));
        assert!(rendered.contains("tool calls by tool: bash=25"));
        assert!(rendered.contains("shell command categories:"));
        assert!(rendered.contains("repeated_reads=3"));
        assert!(rendered.contains("optimization hints:"));
    }

    #[tokio::test]
    async fn telemetry_correlation_groups_before_and_after_landing_points() {
        let pool = memory_pool().await;
        sqlx::query(
            r#"
            INSERT INTO task_queues (
                id, key, name, managed_source_repository, main_branch, worktree_root, branch_template
            ) VALUES ('queue-1', 'TASK', 'Tasker', '/repo', 'main', '/worktrees', 'tasker/{task_identifier}')
            "#,
        )
        .execute(&pool)
        .await
        .expect("queue");
        for (identifier, sequence, title) in [
            ("TASK-1", 1, "Before duplicate"),
            ("TASK-2", 2, "Landing fix"),
            ("TASK-3", 3, "After failure"),
            ("TASK-4", 4, "After integrating"),
        ] {
            sqlx::query(
                "INSERT INTO tasks (id, task_queue_id, identifier, sequence, title, brief, priority, state) VALUES (?, 'queue-1', ?, ?, ?, 'brief', 'normal', 'done')",
            )
            .bind(identifier)
            .bind(identifier)
            .bind(sequence)
            .bind(title)
            .execute(&pool)
            .await
            .expect("task");
        }
        insert_run(
            &pool,
            "TASK-1",
            "queue-1",
            "before-1",
            "2026-01-01 00:00:00",
            "2026-01-01 00:01:00",
            Some("completed"),
            None,
        )
        .await;
        insert_run(
            &pool,
            "TASK-1",
            "queue-1",
            "before-2",
            "2026-01-01 00:02:00",
            "2026-01-01 00:03:00",
            Some("completed"),
            None,
        )
        .await;
        audit(
            &pool,
            "TASK-2",
            "task.state_transitioned",
            serde_json::json!({"from":"integrating","to":"done"}),
            "2026-01-01 01:00:00",
        )
        .await;
        sqlx::query(
            "INSERT INTO integration_outcomes (id, task_id, agent_run_id, outcome_kind, final_commit, created_at) VALUES ('outcome-1', 'TASK-2', NULL, 'success', 'abc123', '2026-01-01 01:00:00')",
        )
        .execute(&pool)
        .await
        .expect("outcome");
        insert_run(
            &pool,
            "TASK-3",
            "queue-1",
            "after-failed",
            "2026-01-01 02:00:00",
            "2026-01-01 02:01:00",
            Some("failed"),
            Some("launcher timed out"),
        )
        .await;
        audit(
            &pool,
            "TASK-4",
            "task.state_transitioned",
            serde_json::json!({"from":"in_progress","to":"integrating"}),
            "2026-01-01 02:10:00",
        )
        .await;
        insert_run(
            &pool,
            "TASK-4",
            "queue-1",
            "post-integrating",
            "2026-01-01 02:20:00",
            "2026-01-01 02:30:00",
            Some("completed"),
            None,
        )
        .await;
        audit(
            &pool,
            "TASK-4",
            "task.state_transitioned",
            serde_json::json!({"from":"integrating","to":"done"}),
            "2026-01-01 03:10:00",
        )
        .await;

        let summary = correlation_summary(
            &pool,
            &CorrelationOptions {
                queue: "TASK".to_string(),
                landing_tasks: vec!["TASK-2".to_string()],
                landing_commits: vec!["abc123".to_string()],
                landing_timestamps: vec!["2026-01-01 01:00:00".to_string()],
            },
        )
        .await
        .expect("correlation");

        assert_eq!(summary.landing_points.len(), 3);
        let task_landing = summary
            .landing_points
            .iter()
            .find(|landing| landing.source == "task")
            .expect("task landing");
        assert_eq!(task_landing.before.duplicate_agent_run_waste, 1);
        assert_eq!(task_landing.after.failed_agent_runs, 1);
        assert_eq!(task_landing.after.post_integrating_agent_runs, 1);
        assert_eq!(
            task_landing.after.average_integrating_wait_seconds,
            Some(3600)
        );
        assert!(task_landing
            .interpretations
            .iter()
            .any(|line| line.contains("appears historical")));
        let json = serde_json::to_value(&summary).expect("json");
        assert_eq!(json["queue"], "TASK");
        assert!(json["landing_points"]
            .as_array()
            .expect("landing points")
            .iter()
            .any(|point| point["before"]["duplicate_agent_run_waste"] == 1));
        assert!(render_correlation_summary(&summary).contains("Focus current work"));
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

    async fn seed_lifecycle_task(
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
        let task_id = seed_lifecycle_task(&pool, "TASK-1", "Finished task", "done").await;
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
        let first = seed_lifecycle_task(&pool, "TASK-1", "Incomplete task", "integrating").await;
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
        let _second = seed_lifecycle_task(&pool, "TASK-2", "Missing history task", "done").await;

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

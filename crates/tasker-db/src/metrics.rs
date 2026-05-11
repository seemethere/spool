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

pub async fn get_agent_run(pool: &SqlitePool, run_id: &str) -> Result<Option<AgentRun>> {
    let select_run_sql = agent_run_select_sql("WHERE id = ?");
    sqlx::query_as::<_, AgentRun>(&select_run_sql)
        .bind(run_id)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("failed to load Agent Run {run_id}"))
}

pub async fn upsert_launcher_session_data(
    pool: &SqlitePool,
    agent_run_id: &str,
    input: &UpsertLauncherSessionData,
    actor: &Actor,
) -> Result<LauncherSessionData> {
    validate_actor(actor)?;
    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let run_exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM agent_runs WHERE id = ?")
        .bind(agent_run_id)
        .fetch_optional(&mut *tx)
        .await
        .with_context(|| format!("failed to load Agent Run {agent_run_id}"))?;
    if run_exists.is_none() {
        anyhow::bail!("Agent Run {agent_run_id} not found");
    }
    sqlx::query(
        r#"
        INSERT INTO launcher_session_data (
            agent_run_id, launcher_kind, session_id, model, provider, started_at, finished_at,
            final_status, transcript_path, raw_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(agent_run_id) DO UPDATE SET
            launcher_kind = excluded.launcher_kind,
            session_id = excluded.session_id,
            model = excluded.model,
            provider = excluded.provider,
            started_at = excluded.started_at,
            finished_at = excluded.finished_at,
            final_status = excluded.final_status,
            transcript_path = excluded.transcript_path,
            raw_json = excluded.raw_json,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(agent_run_id)
    .bind(&input.launcher_kind)
    .bind(&input.session_id)
    .bind(&input.model)
    .bind(&input.provider)
    .bind(&input.started_at)
    .bind(&input.finished_at)
    .bind(&input.final_status)
    .bind(&input.transcript_path)
    .bind(&input.raw_json)
    .execute(&mut *tx)
    .await
    .context("failed to upsert Launcher Session Data")?;
    append_audit_event_in_tx(
        &mut tx,
        actor,
        "agent_run.launcher_session_data_recorded",
        "agent_run",
        agent_run_id,
        serde_json::json!({
            "launcher_kind": input.launcher_kind,
            "session_id": input.session_id,
            "final_status": input.final_status,
            "transcript_path": input.transcript_path,
        }),
    )
    .await?;
    tx.commit().await.context("failed to commit transaction")?;
    refresh_agent_run_metrics(pool, agent_run_id).await?;
    get_launcher_session_data(pool, agent_run_id)
        .await?
        .with_context(|| format!("Launcher Session Data for Agent Run {agent_run_id} not found"))
}

pub async fn get_launcher_session_data(
    pool: &SqlitePool,
    agent_run_id: &str,
) -> Result<Option<LauncherSessionData>> {
    sqlx::query_as::<_, LauncherSessionData>(
        r#"
        SELECT agent_run_id, launcher_kind, session_id, model, provider, started_at, finished_at,
               final_status, transcript_path, raw_json, created_at, updated_at
        FROM launcher_session_data
        WHERE agent_run_id = ?
        "#,
    )
    .bind(agent_run_id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to load Launcher Session Data for Agent Run {agent_run_id}"))
}

pub async fn get_agent_run_metrics(
    pool: &SqlitePool,
    agent_run_id: &str,
) -> Result<Option<AgentRunMetrics>> {
    sqlx::query_as::<_, AgentRunMetrics>(
        r#"
        SELECT agent_run_id, duration_ms, launcher_kind, final_status, exit_code, timed_out,
               unattended_question_detected, blocking_ui_detected, transcript_path,
               transcript_byte_size, transcript_jsonl_event_count, input_tokens, output_tokens,
               total_tokens, cache_read_tokens, cache_write_tokens, tool_call_count, tool_error_count,
               repeated_failed_tool_attempt_count, tool_call_counts_json, repeated_read_count,
               repeated_tasker_context_fetch_count, shell_command_counts_json,
               assistant_turn_count, user_turn_count, max_context_tokens, efficiency_hints_json,
               warnings_json, created_at, updated_at
        FROM agent_run_metrics
        WHERE agent_run_id = ?
        "#,
    )
    .bind(agent_run_id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to load Agent Run metrics for Agent Run {agent_run_id}"))
}

pub async fn compute_agent_run_metrics(
    pool: &SqlitePool,
    agent_run_id: &str,
) -> Result<Option<ComputedAgentRunMetrics>> {
    let Some(run) = get_agent_run(pool, agent_run_id).await? else {
        anyhow::bail!("Agent Run {agent_run_id} not found");
    };
    if run.outcome.is_none() {
        return Ok(None);
    }
    let session = get_launcher_session_data(pool, agent_run_id).await?;
    let mut summary = AgentRunMetricsSummary::default();
    if let Some(session) = &session {
        summary.launcher_kind = session.launcher_kind.clone();
        summary.final_status = session.final_status.clone().or_else(|| run.outcome.clone());
        summary.transcript_path = session.transcript_path.clone();
        summary.observe_launcher_raw_json(session.raw_json.as_deref());
        if let Some(path) = &session.transcript_path {
            summary.observe_transcript(Path::new(path));
        }
    } else {
        summary.launcher_kind = run.launcher_kind.clone();
        summary.final_status = run.outcome.clone();
        summary
            .warnings
            .push("Launcher Session Data not recorded".to_string());
    }
    let warnings_json = serde_json::to_string(&summary.warnings)
        .context("failed to serialize Agent Run metrics warnings")?;
    let duration_ms: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT CAST((julianday(finished_at) - julianday(created_at)) * 86400000 AS INTEGER)
        FROM agent_runs
        WHERE id = ? AND finished_at IS NOT NULL
        "#,
    )
    .bind(agent_run_id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to compute Agent Run duration for Agent Run {agent_run_id}"))?
    .flatten();
    let tool_call_counts_json = summary.tool_call_counts_json()?;
    let shell_command_counts_json = summary.shell_command_counts_json()?;
    let efficiency_hints_json = summary.efficiency_hints_json()?;
    Ok(Some(ComputedAgentRunMetrics {
        agent_run_id: agent_run_id.to_string(),
        duration_ms,
        launcher_kind: summary.launcher_kind,
        final_status: summary.final_status,
        exit_code: summary.exit_code,
        timed_out: summary.timed_out.map(bool_to_i64),
        unattended_question_detected: summary.unattended_question_detected.map(bool_to_i64),
        blocking_ui_detected: summary.blocking_ui_detected.map(bool_to_i64),
        transcript_path: summary.transcript_path,
        transcript_byte_size: summary.transcript_byte_size,
        transcript_jsonl_event_count: summary.transcript_jsonl_event_count,
        input_tokens: summary.input_tokens,
        output_tokens: summary.output_tokens,
        total_tokens: summary.total_tokens,
        cache_read_tokens: summary.cache_read_tokens,
        cache_write_tokens: summary.cache_write_tokens,
        tool_call_count: summary.tool_call_count,
        tool_error_count: summary.tool_error_count,
        repeated_failed_tool_attempt_count: summary.repeated_failed_tool_attempt_count,
        tool_call_counts_json,
        repeated_read_count: summary.repeated_read_count,
        repeated_tasker_context_fetch_count: summary.repeated_tasker_context_fetch_count,
        shell_command_counts_json,
        assistant_turn_count: summary.assistant_turn_count,
        user_turn_count: summary.user_turn_count,
        max_context_tokens: summary.max_context_tokens,
        efficiency_hints_json,
        warnings_json,
    }))
}

pub async fn refresh_agent_run_metrics(
    pool: &SqlitePool,
    agent_run_id: &str,
) -> Result<Option<AgentRunMetrics>> {
    let Some(metrics) = compute_agent_run_metrics(pool, agent_run_id).await? else {
        return Ok(None);
    };
    sqlx::query(
        r#"
        INSERT INTO agent_run_metrics (
            agent_run_id, duration_ms, launcher_kind, final_status, exit_code, timed_out,
            unattended_question_detected, blocking_ui_detected, transcript_path,
            transcript_byte_size, transcript_jsonl_event_count, input_tokens, output_tokens,
            total_tokens, cache_read_tokens, cache_write_tokens, tool_call_count, tool_error_count,
            repeated_failed_tool_attempt_count, tool_call_counts_json, repeated_read_count,
            repeated_tasker_context_fetch_count, shell_command_counts_json,
            assistant_turn_count, user_turn_count, max_context_tokens, efficiency_hints_json, warnings_json
        )
        SELECT
            agent_runs.id,
            ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
        FROM agent_runs
        WHERE agent_runs.id = ? AND agent_runs.outcome IS NOT NULL
        ON CONFLICT(agent_run_id) DO UPDATE SET
            duration_ms = excluded.duration_ms,
            launcher_kind = excluded.launcher_kind,
            final_status = excluded.final_status,
            exit_code = excluded.exit_code,
            timed_out = excluded.timed_out,
            unattended_question_detected = excluded.unattended_question_detected,
            blocking_ui_detected = excluded.blocking_ui_detected,
            transcript_path = excluded.transcript_path,
            transcript_byte_size = excluded.transcript_byte_size,
            transcript_jsonl_event_count = excluded.transcript_jsonl_event_count,
            input_tokens = excluded.input_tokens,
            output_tokens = excluded.output_tokens,
            total_tokens = excluded.total_tokens,
            cache_read_tokens = excluded.cache_read_tokens,
            cache_write_tokens = excluded.cache_write_tokens,
            tool_call_count = excluded.tool_call_count,
            tool_error_count = excluded.tool_error_count,
            repeated_failed_tool_attempt_count = excluded.repeated_failed_tool_attempt_count,
            tool_call_counts_json = excluded.tool_call_counts_json,
            repeated_read_count = excluded.repeated_read_count,
            repeated_tasker_context_fetch_count = excluded.repeated_tasker_context_fetch_count,
            shell_command_counts_json = excluded.shell_command_counts_json,
            assistant_turn_count = excluded.assistant_turn_count,
            user_turn_count = excluded.user_turn_count,
            max_context_tokens = excluded.max_context_tokens,
            efficiency_hints_json = excluded.efficiency_hints_json,
            warnings_json = excluded.warnings_json,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(metrics.duration_ms)
    .bind(&metrics.launcher_kind)
    .bind(&metrics.final_status)
    .bind(metrics.exit_code)
    .bind(metrics.timed_out)
    .bind(metrics.unattended_question_detected)
    .bind(metrics.blocking_ui_detected)
    .bind(&metrics.transcript_path)
    .bind(metrics.transcript_byte_size)
    .bind(metrics.transcript_jsonl_event_count)
    .bind(metrics.input_tokens)
    .bind(metrics.output_tokens)
    .bind(metrics.total_tokens)
    .bind(metrics.cache_read_tokens)
    .bind(metrics.cache_write_tokens)
    .bind(metrics.tool_call_count)
    .bind(metrics.tool_error_count)
    .bind(metrics.repeated_failed_tool_attempt_count)
    .bind(&metrics.tool_call_counts_json)
    .bind(metrics.repeated_read_count)
    .bind(metrics.repeated_tasker_context_fetch_count)
    .bind(&metrics.shell_command_counts_json)
    .bind(metrics.assistant_turn_count)
    .bind(metrics.user_turn_count)
    .bind(metrics.max_context_tokens)
    .bind(&metrics.efficiency_hints_json)
    .bind(&metrics.warnings_json)
    .bind(agent_run_id)
    .execute(pool)
    .await
    .with_context(|| format!("failed to persist Agent Run metrics for Agent Run {agent_run_id}"))?;
    get_agent_run_metrics(pool, agent_run_id).await
}

#[derive(Debug, Default)]
struct AgentRunMetricsSummary {
    launcher_kind: String,
    final_status: Option<String>,
    exit_code: Option<i64>,
    timed_out: Option<bool>,
    unattended_question_detected: Option<bool>,
    blocking_ui_detected: Option<bool>,
    transcript_path: Option<String>,
    transcript_byte_size: Option<i64>,
    transcript_jsonl_event_count: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    total_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    tool_call_count: Option<i64>,
    tool_error_count: Option<i64>,
    repeated_failed_tool_attempt_count: Option<i64>,
    tool_call_counts: std::collections::BTreeMap<String, i64>,
    repeated_read_count: Option<i64>,
    repeated_tasker_context_fetch_count: Option<i64>,
    shell_command_counts: std::collections::BTreeMap<String, i64>,
    read_paths: std::collections::HashMap<String, i64>,
    tasker_context_fetch_signatures: std::collections::HashMap<String, i64>,
    seen_tool_call_ids: std::collections::HashSet<String>,
    seen_tool_detail_ids: std::collections::HashSet<String>,
    assistant_turn_count: Option<i64>,
    user_turn_count: Option<i64>,
    max_context_tokens: Option<i64>,
    failed_tool_signatures: std::collections::HashMap<String, i64>,
    warnings: Vec<String>,
}

fn is_success_final_status(status: Option<&str>) -> bool {
    matches!(status, Some("completed" | "succeeded" | "success" | "done"))
}

fn is_blocking_extension_ui_method(method: &str) -> bool {
    matches!(method, "confirm" | "input" | "select" | "editor")
}

impl AgentRunMetricsSummary {
    fn observe_launcher_raw_json(&mut self, raw_json: Option<&str>) {
        let Some(raw_json) = raw_json else { return };
        match serde_json::from_str::<serde_json::Value>(raw_json) {
            Ok(value) => {
                self.exit_code = self.exit_code.or_else(|| json_i64(&value, &["exit_code"]));
                self.timed_out = self.timed_out.or_else(|| json_bool(&value, &["timed_out"]));
                self.unattended_question_detected = self
                    .unattended_question_detected
                    .or_else(|| json_bool(&value, &["unattended_question_detected"]));
                self.observe_token_usage(&value);
            }
            Err(error) => self.warnings.push(format!(
                "ignored malformed Launcher Session Data raw JSON: {error}"
            )),
        }
    }

    fn observe_transcript(&mut self, path: &Path) {
        match fs::metadata(path) {
            Ok(metadata) => self.transcript_byte_size = Some(metadata.len() as i64),
            Err(error) => {
                self.warnings.push(format!(
                    "could not stat Run Transcript {}: {error}",
                    path.display()
                ));
            }
        }
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                self.warnings.push(format!(
                    "could not read Run Transcript {}: {error}",
                    path.display()
                ));
                return;
            }
        };
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(value) => {
                    self.transcript_jsonl_event_count =
                        Some(self.transcript_jsonl_event_count.unwrap_or(0) + 1);
                    self.observe_transcript_record(&value);
                }
                Err(error) => self.warnings.push(format!(
                    "ignored malformed Run Transcript line {}: {error}",
                    index + 1
                )),
            }
        }
    }

    fn observe_transcript_record(&mut self, value: &serde_json::Value) {
        self.observe_event(value);
        self.exit_code = self.exit_code.or_else(|| json_i64(value, &["status"]));
        self.timed_out = self.timed_out.or_else(|| json_bool(value, &["timed_out"]));
        self.unattended_question_detected = self
            .unattended_question_detected
            .or_else(|| json_bool(value, &["unattended_question_detected"]));
        for field in ["stdout", "stderr"] {
            if let Some(text) = value.get(field).and_then(|value| value.as_str()) {
                for (index, line) in text.lines().enumerate() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() || !trimmed.starts_with('{') {
                        continue;
                    }
                    match serde_json::from_str::<serde_json::Value>(trimmed) {
                        Ok(value) => self.observe_event(&value),
                        Err(error) => self.warnings.push(format!(
                            "ignored malformed JSON event in {field} line {}: {error}",
                            index + 1
                        )),
                    }
                }
            }
        }
    }

    fn observe_event(&mut self, value: &serde_json::Value) {
        if value.get("type").and_then(|value| value.as_str()) == Some("extension_ui_request") {
            let method = value
                .get("method")
                .or_else(|| value.get("method_name"))
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            if is_blocking_extension_ui_method(method) {
                self.blocking_ui_detected = Some(true);
            }
        }
        if value.get("event").and_then(|value| value.as_str()) == Some("question")
            && !is_success_final_status(self.final_status.as_deref())
        {
            self.unattended_question_detected = Some(true);
        }
        self.observe_roles_and_usage(value);
        self.observe_tool_event(value);
        self.observe_nested_tool_events(value);
    }

    fn observe_nested_tool_events(&mut self, value: &serde_json::Value) {
        for path in ["/message/content", "/assistantMessageEvent/partial/content"] {
            if let Some(content) = value.pointer(path).and_then(|value| value.as_array()) {
                for item in content {
                    self.observe_tool_event(item);
                }
            }
        }
    }

    fn observe_roles_and_usage(&mut self, value: &serde_json::Value) {
        if let Some(role) = value.get("role").and_then(|value| value.as_str()) {
            match role {
                "assistant" => {
                    self.assistant_turn_count = Some(self.assistant_turn_count.unwrap_or(0) + 1)
                }
                "user" => self.user_turn_count = Some(self.user_turn_count.unwrap_or(0) + 1),
                _ => {}
            }
        }
        self.observe_token_usage(value);
    }

    fn observe_token_usage(&mut self, value: &serde_json::Value) {
        if let Some(input) = first_json_i64(
            value,
            &[
                &["input_tokens"],
                &["inputTokens"],
                &["usage", "input_tokens"],
                &["usage", "inputTokens"],
                &["usage", "input"],
                &["usage", "prompt_tokens"],
                &["message", "usage", "input"],
                &["message", "usage", "input_tokens"],
                &["message", "usage", "inputTokens"],
                &["assistantMessageEvent", "partial", "usage", "input"],
                &["assistantMessageEvent", "partial", "usage", "input_tokens"],
                &["assistantMessageEvent", "partial", "usage", "inputTokens"],
            ],
        ) {
            self.input_tokens = Some(self.input_tokens.unwrap_or(0).max(input));
        }
        if let Some(output) = first_json_i64(
            value,
            &[
                &["output_tokens"],
                &["outputTokens"],
                &["usage", "output_tokens"],
                &["usage", "outputTokens"],
                &["usage", "output"],
                &["usage", "completion_tokens"],
                &["message", "usage", "output"],
                &["message", "usage", "output_tokens"],
                &["message", "usage", "outputTokens"],
                &["assistantMessageEvent", "partial", "usage", "output"],
                &["assistantMessageEvent", "partial", "usage", "output_tokens"],
                &["assistantMessageEvent", "partial", "usage", "outputTokens"],
            ],
        ) {
            self.output_tokens = Some(self.output_tokens.unwrap_or(0).max(output));
        }
        if let Some(total) = first_json_i64(
            value,
            &[
                &["total_tokens"],
                &["totalTokens"],
                &["usage", "total_tokens"],
                &["usage", "totalTokens"],
                &["message", "usage", "total_tokens"],
                &["message", "usage", "totalTokens"],
                &["assistantMessageEvent", "partial", "usage", "total_tokens"],
                &["assistantMessageEvent", "partial", "usage", "totalTokens"],
            ],
        ) {
            self.total_tokens = Some(self.total_tokens.unwrap_or(0).max(total));
            self.max_context_tokens = Some(self.max_context_tokens.unwrap_or(0).max(total));
        }
        if let Some(cache_read) = first_json_i64(
            value,
            &[
                &["cache_read_tokens"],
                &["cacheReadTokens"],
                &["usage", "cache_read_tokens"],
                &["usage", "cacheReadTokens"],
                &["usage", "cacheRead"],
                &["message", "usage", "cacheRead"],
                &["assistantMessageEvent", "partial", "usage", "cacheRead"],
            ],
        ) {
            self.cache_read_tokens = Some(self.cache_read_tokens.unwrap_or(0).max(cache_read));
        }
        if let Some(cache_write) = first_json_i64(
            value,
            &[
                &["cache_write_tokens"],
                &["cacheWriteTokens"],
                &["usage", "cache_write_tokens"],
                &["usage", "cacheWriteTokens"],
                &["usage", "cacheWrite"],
                &["message", "usage", "cacheWrite"],
                &["assistantMessageEvent", "partial", "usage", "cacheWrite"],
            ],
        ) {
            self.cache_write_tokens = Some(self.cache_write_tokens.unwrap_or(0).max(cache_write));
        }
        if let Some(context) = first_json_i64(
            value,
            &[
                &["context_tokens"],
                &["contextTokens"],
                &["max_context_tokens"],
                &["maxContextTokens"],
                &["usage", "context_tokens"],
                &["usage", "contextTokens"],
                &["usage", "max_context_tokens"],
                &["usage", "maxContextTokens"],
                &["message", "usage", "context_tokens"],
                &["message", "usage", "contextTokens"],
                &[
                    "assistantMessageEvent",
                    "partial",
                    "usage",
                    "context_tokens",
                ],
                &["assistantMessageEvent", "partial", "usage", "contextTokens"],
            ],
        ) {
            self.max_context_tokens = Some(self.max_context_tokens.unwrap_or(0).max(context));
        }
    }

    fn observe_tool_event(&mut self, value: &serde_json::Value) {
        let type_text = value
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let tool_name = tool_name(value);
        let has_tool_name = tool_name.is_some();
        let is_tool_delta = type_text.contains("toolcall_delta")
            || type_text.contains("tool_call_delta")
            || type_text.contains("tool_delta");
        let is_tool_call = !is_tool_delta
            && ((type_text.contains("tool")
                && (type_text.contains("call")
                    || type_text.contains("use")
                    || type_text.contains("start")
                    || type_text.contains("execution")))
                || type_text == "function_call"
                || value.get("function_call").is_some());
        if is_tool_call {
            let call_id = tool_call_id(value);
            let already_counted = call_id
                .as_ref()
                .is_some_and(|call_id| self.seen_tool_call_ids.contains(call_id));
            let name = tool_name.unwrap_or_else(|| "unknown".to_string());
            if !already_counted {
                if let Some(call_id) = &call_id {
                    self.seen_tool_call_ids.insert(call_id.clone());
                }
                self.tool_call_count = Some(self.tool_call_count.unwrap_or(0) + 1);
                *self.tool_call_counts.entry(name.clone()).or_insert(0) += 1;
            }
            let details_already_observed = call_id
                .as_ref()
                .is_some_and(|call_id| self.seen_tool_detail_ids.contains(call_id));
            if !details_already_observed && self.observe_tool_call_details(&name, value) {
                if let Some(call_id) = call_id {
                    self.seen_tool_detail_ids.insert(call_id);
                }
            }
        }
        let status = value
            .get("status")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let is_error = value
            .get("is_error")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
            || status == "error"
            || status == "failed"
            || type_text.contains("error");
        if is_error && (has_tool_name || type_text.contains("tool")) {
            self.tool_error_count = Some(self.tool_error_count.unwrap_or(0) + 1);
            let signature = tool_signature(value);
            let count = self.failed_tool_signatures.entry(signature).or_insert(0);
            *count += 1;
            if *count > 1 {
                self.repeated_failed_tool_attempt_count =
                    Some(self.repeated_failed_tool_attempt_count.unwrap_or(0) + 1);
            }
        }
    }

    fn observe_tool_call_details(&mut self, name: &str, value: &serde_json::Value) -> bool {
        let canonical = canonical_tool_name(name);
        if canonical == "read" {
            if let Some(path) = tool_string_arg(value, &["path", "file", "filename"]) {
                let count = self.read_paths.entry(path).or_insert(0);
                *count += 1;
                if *count > 1 {
                    self.repeated_read_count = Some(self.repeated_read_count.unwrap_or(0) + 1);
                }
                return true;
            }
            return false;
        }
        if canonical == "bash" {
            if let Some(command) = tool_string_arg(value, &["command", "cmd"]) {
                let category = shell_command_category(&command);
                *self
                    .shell_command_counts
                    .entry(category.to_string())
                    .or_insert(0) += 1;
                if let Some(signature) = tasker_context_fetch_signature(&command) {
                    let count = self
                        .tasker_context_fetch_signatures
                        .entry(signature)
                        .or_insert(0);
                    *count += 1;
                    if *count > 1 {
                        self.repeated_tasker_context_fetch_count =
                            Some(self.repeated_tasker_context_fetch_count.unwrap_or(0) + 1);
                    }
                }
                return true;
            }
            return false;
        } else if is_tasker_context_tool(&canonical) {
            let count = self
                .tasker_context_fetch_signatures
                .entry(format!("tool:{canonical}"))
                .or_insert(0);
            *count += 1;
            if *count > 1 {
                self.repeated_tasker_context_fetch_count =
                    Some(self.repeated_tasker_context_fetch_count.unwrap_or(0) + 1);
            }
            return true;
        }
        false
    }

    fn tool_call_counts_json(&self) -> Result<String> {
        serde_json::to_string(&self.tool_call_counts)
            .context("failed to serialize Agent Run per-tool counts")
    }

    fn shell_command_counts_json(&self) -> Result<String> {
        serde_json::to_string(&self.shell_command_counts)
            .context("failed to serialize Agent Run shell command counts")
    }

    fn efficiency_hints_json(&self) -> Result<String> {
        let mut hints = Vec::new();
        if self.tool_call_count.unwrap_or(0) >= 30 {
            hints.push("excessive tool calls".to_string());
        }
        if self.repeated_failed_tool_attempt_count.unwrap_or(0) > 0 {
            hints.push("repeated failed tool attempts".to_string());
        }
        if self.repeated_read_count.unwrap_or(0) > 0 {
            hints.push("repeated file reads".to_string());
        }
        if self.repeated_tasker_context_fetch_count.unwrap_or(0) > 0 {
            hints.push("repeated Tasker context fetches".to_string());
        }
        if self.transcript_byte_size.unwrap_or(0) >= 10_000_000 {
            hints.push("large transcript/proxy output volume".to_string());
        }
        if self.max_context_tokens.unwrap_or(0) >= 100_000 {
            hints.push("large context growth".to_string());
        }
        // UI interaction signals are emitted as dedicated metrics, not generic optimization
        // hints. `blocking_ui_detected` means a blocking extension UI method was observed
        // (`confirm`, `input`, `select`, or `editor`). `unattended_question_detected` means
        // explicit launcher metadata reported an unattended question, or a question event was
        // observed on a non-successful run. Benign
        // fire-and-forget UI such as `notify` should not become an efficiency hint.
        if self.tool_error_count.unwrap_or(0) >= 5 {
            hints.push("validation/tool loop".to_string());
        }
        serde_json::to_string(&hints).context("failed to serialize Agent Run efficiency hints")
    }
}

fn tool_name(value: &serde_json::Value) -> Option<String> {
    let raw = value
        .get("tool_name")
        .or_else(|| value.get("toolName"))
        .or_else(|| value.get("tool"))
        .or_else(|| value.get("name"))
        .and_then(|value| value.as_str())
        .or_else(|| {
            value
                .pointer("/function/name")
                .and_then(|value| value.as_str())
        })
        .or_else(|| {
            value
                .pointer("/function_call/name")
                .and_then(|value| value.as_str())
        })
        .or_else(|| {
            value
                .pointer("/toolCall/name")
                .and_then(|value| value.as_str())
        })?;
    Some(sanitize_metric_key(raw))
}

fn tool_call_id(value: &serde_json::Value) -> Option<String> {
    value
        .get("id")
        .or_else(|| value.get("tool_call_id"))
        .or_else(|| value.get("toolCallId"))
        .or_else(|| value.get("call_id"))
        .and_then(|value| value.as_str())
        .map(sanitize_metric_key)
}

fn sanitize_metric_key(raw: &str) -> String {
    let lowered = raw.trim().to_ascii_lowercase();
    let sanitized: String = lowered
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn canonical_tool_name(name: &str) -> String {
    name.rsplit(['.', ':']).next().unwrap_or(name).to_string()
}

fn tool_args(value: &serde_json::Value) -> Option<&serde_json::Value> {
    value
        .get("args")
        .or_else(|| value.get("arguments"))
        .or_else(|| value.get("input"))
        .or_else(|| value.get("partialJson"))
        .or_else(|| value.pointer("/function/arguments"))
        .or_else(|| value.pointer("/function_call/arguments"))
}

fn tool_string_arg(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    let args = tool_args(value)?;
    if let Some(text) = args.as_str() {
        if keys.iter().any(|key| matches!(*key, "command" | "cmd")) {
            return Some(text.to_string());
        }
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) {
            return string_arg_from_object(&parsed, keys);
        }
        return None;
    }
    string_arg_from_object(args, keys)
}

fn string_arg_from_object(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(|value| value.as_str()) {
            return Some(text.trim().to_string());
        }
    }
    None
}

fn shell_command_category(command: &str) -> &'static str {
    let lowered = command.trim_start().to_ascii_lowercase();
    let tokens = shell_command_tokens(&lowered);
    if tokens.iter().any(|token| token == "tasker-local")
        || is_direct_tasker_invocation(&lowered)
        || tokens.windows(4).any(|window| {
            window[0] == "cargo"
                && window[1] == "run"
                && matches!(window[2].as_str(), "-p" | "--package")
                && window[3] == "tasker-cli"
        })
    {
        "tasker_cli"
    } else if has_shell_invocation(&tokens, &["cargo"])
        || tokens
            .iter()
            .any(|token| matches!(token.as_str(), "cargo-nextest" | "nextest"))
    {
        "cargo"
    } else if has_shell_invocation(&tokens, &["git"])
        || tokens
            .iter()
            .any(|token| matches!(token.as_str(), "gh" | "git-lfs"))
    {
        "git"
    } else if has_shell_invocation(
        &tokens,
        &[
            "ps",
            "pgrep",
            "pkill",
            "kill",
            "killall",
            "jobs",
            "lsof",
            "top",
            "htop",
            "launchctl",
            "supervisorctl",
        ],
    ) {
        "process"
    } else if has_shell_invocation(
        &tokens,
        &["rg", "ripgrep", "grep", "egrep", "fgrep", "find", "fd"],
    ) {
        "search"
    } else if has_shell_invocation(
        &tokens,
        &[
            "ls", "pwd", "tree", "stat", "du", "df", "realpath", "dirname", "basename", "mkdir",
            "rmdir", "cp", "mv", "rm", "touch",
        ],
    ) {
        "filesystem"
    } else if has_shell_invocation(
        &tokens,
        &[
            "npm", "pnpm", "yarn", "bun", "make", "just", "cmake", "ninja", "node", "tsc", "vite",
            "webpack",
        ],
    ) {
        "package_build"
    } else if has_shell_invocation(&tokens, &["sqlite3", "sqlx"])
        || tokens
            .iter()
            .any(|token| matches!(token.as_str(), "sqlite" | "sqlite-utils"))
    {
        "sqlite"
    } else if has_shell_invocation(
        &tokens,
        &[
            "jq", "sed", "awk", "cut", "sort", "uniq", "wc", "head", "tail", "tr", "xargs", "tee",
            "cat",
        ],
    ) {
        "text_processing"
    } else {
        "miscellaneous"
    }
}

fn is_direct_tasker_invocation(command: &str) -> bool {
    let trimmed = command.trim_start_matches(|ch: char| ch.is_ascii_whitespace() || ch == '(');
    trimmed.starts_with("tasker ")
        || trimmed.contains("&& tasker ")
        || trimmed.contains("; tasker ")
        || trimmed.contains("| tasker ")
}

fn shell_command_tokens(command: &str) -> Vec<String> {
    command
        .split(|ch: char| {
            ch.is_ascii_whitespace() || matches!(ch, ';' | '|' | '&' | '(' | ')' | '<' | '>' | '`')
        })
        .filter_map(|raw| {
            let trimmed =
                raw.trim_matches(|ch: char| matches!(ch, '"' | '\'' | '[' | ']' | '{' | '}' | ','));
            if trimmed.is_empty() || trimmed.contains('=') && !trimmed.starts_with('-') {
                return None;
            }
            let basename = trimmed.rsplit('/').next().unwrap_or(trimmed);
            let command_name = basename.strip_suffix(':').unwrap_or(basename);
            if command_name.is_empty() {
                None
            } else {
                Some(command_name.to_string())
            }
        })
        .collect()
}

fn has_shell_invocation(tokens: &[String], commands: &[&str]) -> bool {
    tokens
        .iter()
        .any(|token| commands.contains(&token.as_str()))
}

fn tasker_context_fetch_signature(command: &str) -> Option<String> {
    let lowered = command.to_ascii_lowercase();
    let normalized = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
    let context_kind = if normalized.contains("tasker-local task show")
        || normalized.contains("tasker task show")
        || normalized.contains("task show")
    {
        "task_show"
    } else if normalized.contains("tasker-local queue show")
        || normalized.contains("tasker queue show")
        || normalized.contains("queue show")
    {
        "queue_show"
    } else if normalized.contains("tasker-local run show")
        || normalized.contains("tasker run show")
        || normalized.contains("run show")
    {
        "run_show"
    } else if normalized.contains("tasker-local status")
        || normalized.contains("tasker status")
        || normalized == "status"
    {
        "status"
    } else {
        return None;
    };
    Some(context_kind.to_string())
}

fn is_tasker_context_tool(canonical: &str) -> bool {
    canonical.contains("get_task")
        || canonical.contains("task_context")
        || canonical.contains("task_show")
        || canonical.contains("queue_show")
        || canonical == "status"
}

fn tool_signature(value: &serde_json::Value) -> String {
    let name = tool_name(value).unwrap_or_else(|| "unknown".to_string());
    let args = value
        .get("args")
        .or_else(|| value.get("arguments"))
        .or_else(|| value.get("input"))
        .map(|value| value.to_string())
        .unwrap_or_default();
    format!("{name}:{args}")
}

fn bool_to_i64(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn json_i64(value: &serde_json::Value, path: &[&str]) -> Option<i64> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_i64()
}

fn json_bool(value: &serde_json::Value, path: &[&str]) -> Option<bool> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_bool()
}

fn first_json_i64(value: &serde_json::Value, paths: &[&[&str]]) -> Option<i64> {
    paths.iter().find_map(|path| json_i64(value, path))
}

pub async fn get_agent_run_detail(
    pool: &SqlitePool,
    run_id: &str,
) -> Result<Option<AgentRunDetail>> {
    let Some(run) = get_agent_run(pool, run_id).await? else {
        return Ok(None);
    };
    agent_run_detail_for_run(pool, run).await.map(Some)
}

pub async fn get_latest_agent_run_detail_for_task(
    pool: &SqlitePool,
    identifier: &str,
) -> Result<Option<AgentRunDetail>> {
    let select_run_sql = agent_run_select_sql(
        r#"
        JOIN tasks ON tasks.id = agent_runs.task_id
        WHERE tasks.identifier = ?
        ORDER BY agent_runs.created_at DESC, agent_runs.id DESC
        LIMIT 1
        "#,
    );
    let Some(run) = sqlx::query_as::<_, AgentRun>(&select_run_sql)
        .bind(identifier)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("failed to load latest Agent Run for Task {identifier}"))?
    else {
        return Ok(None);
    };
    agent_run_detail_for_run(pool, run).await.map(Some)
}

async fn agent_run_detail_for_run(pool: &SqlitePool, run: AgentRun) -> Result<AgentRunDetail> {
    let identifier: String = sqlx::query_scalar("SELECT identifier FROM tasks WHERE id = ?")
        .bind(&run.task_id)
        .fetch_one(pool)
        .await
        .with_context(|| format!("failed to load Task for Agent Run {}", run.id))?;
    let task = get_task_detail(pool, &identifier)
        .await?
        .with_context(|| format!("Task {identifier} for Agent Run {} not found", run.id))?;
    let launcher_session_data = get_launcher_session_data(pool, &run.id).await?;
    let metrics = get_agent_run_metrics(pool, &run.id).await?;
    Ok(AgentRunDetail {
        run,
        task,
        launcher_session_data,
        metrics,
    })
}

#[cfg(test)]
mod shell_command_category_tests {
    use super::shell_command_category;

    #[test]
    fn classifies_common_dogfood_shell_commands_without_raw_command_storage() {
        let cases = [
            ("bin/tasker-local task show TASKER-86", "tasker_cli"),
            ("cargo test -p tasker-db agent_run_metrics", "cargo"),
            (
                "cargo clippy -p tasker-cli --all-targets -- -D warnings",
                "cargo",
            ),
            ("git status --short", "git"),
            ("rg telemetry crates", "search"),
            ("find crates -name '*.rs'", "search"),
            ("ls -la && stat Cargo.toml", "filesystem"),
            (
                "sqlite3 .tasker/data/tasker.db 'select count(*) from tasks'",
                "sqlite",
            ),
            ("ps aux | grep tasker", "process"),
            ("pgrep -fl tasker", "process"),
            ("jq '.efficiency' summary.json", "text_processing"),
            ("sed -n '1,20p' CONTEXT.md", "text_processing"),
            ("awk '{print $1}' counts.txt", "text_processing"),
            ("pnpm build", "package_build"),
            ("make test", "package_build"),
            ("python scripts/one_off.py", "miscellaneous"),
        ];

        for (command, expected) in cases {
            assert_eq!(shell_command_category(command), expected, "{command}");
        }
    }
}

use std::{
    fs,
    io::{Result, Write},
    path::Path,
    process::Command as ProcessCommand,
};

use serde::Serialize;

pub fn write_task_detail(mut writer: impl Write, detail: &tasker_db::TaskDetail) -> Result<()> {
    writeln!(writer, "Task: {}", detail.task.identifier)?;
    writeln!(writer, "title: {}", detail.task.title)?;
    writeln!(writer, "Task Queue: {}", detail.task.task_queue_key)?;
    writeln!(writer, "Task State: {}", detail.task.state)?;
    writeln!(writer, "Priority: {}", detail.task.priority)?;
    writeln!(writer, "review required: {}", detail.task.review_required)?;
    writeln!(
        writer,
        "Validated Base Commit: {}",
        detail
            .task
            .validated_base_commit
            .as_deref()
            .unwrap_or("not recorded")
    )?;
    if !detail.tags.is_empty() {
        writeln!(writer, "tags: {}", detail.tags.join(", "))?;
    }
    writeln!(writer, "\nTask Brief:\n{}", detail.task.brief)?;
    writeln!(writer, "\nAcceptance Criteria:")?;
    for criterion in &detail.acceptance_criteria {
        writeln!(
            writer,
            "  {}. [{}] {}",
            criterion.position, criterion.status, criterion.description
        )?;
        if let Some(reason) = &criterion.waiver_reason {
            writeln!(writer, "     waiver: {reason}")?;
        }
    }
    writeln!(writer, "\nValidation Items:")?;
    for item in &detail.validation_items {
        writeln!(
            writer,
            "  {}. [{}] {}",
            item.position, item.status, item.description
        )?;
        if let Some(reason) = &item.waiver_reason {
            writeln!(writer, "     waiver: {reason}")?;
        }
    }
    writeln!(writer, "\nTask Links:")?;
    if detail.task_links.is_empty() {
        writeln!(writer, "(none)")?;
    } else {
        for link in &detail.task_links {
            let primary = if link.is_primary { " primary" } else { "" };
            let label = link.label.as_deref().unwrap_or("");
            writeln!(
                writer,
                "  [{}{}] {} {}",
                link.kind, primary, link.target, label
            )?;
        }
    }
    writeln!(writer, "\nLocal Worktree Status:")?;
    if let Some(worktree) = primary_task_link_target(&detail.task_links, "local_worktree") {
        let path = Path::new(worktree);
        writeln!(writer, "  path: {worktree}")?;
        writeln!(
            writer,
            "  exists: {}",
            if path.exists() { "yes" } else { "no" }
        )?;
        if path.exists() {
            match git_output(path, &["status", "--porcelain"]) {
                Ok(status) if status.trim().is_empty() => writeln!(writer, "  clean: yes")?,
                Ok(status) => {
                    writeln!(writer, "  clean: no")?;
                    let changed = status.lines().count();
                    writeln!(writer, "  uncommitted paths: {changed}")?;
                }
                Err(error) => writeln!(writer, "  clean: unknown ({error})")?,
            }
            match git_output(path, &["rev-parse", "--abbrev-ref", "HEAD"]) {
                Ok(branch) => writeln!(writer, "  checked-out branch: {}", branch.trim())?,
                Err(error) => writeln!(writer, "  checked-out branch: unknown ({error})")?,
            }
        }
    } else {
        writeln!(writer, "  path: missing Task Link")?;
        writeln!(writer, "  exists: unknown")?;
    }
    if let Some(branch) = primary_task_link_target(&detail.task_links, "task_branch") {
        writeln!(writer, "  Task Branch: {branch}")?;
    } else {
        writeln!(writer, "  Task Branch: missing Task Link")?;
    }
    writeln!(writer, "\nLatest Rework Reason:")?;
    if detail.latest_rework_reason_code.is_none() && detail.latest_rework_reason.is_none() {
        writeln!(writer, "(none)")?;
    } else {
        writeln!(
            writer,
            "  code: {}",
            detail
                .latest_rework_reason_code
                .as_deref()
                .unwrap_or("unknown_legacy")
        )?;
        if let Some(reason) = &detail.latest_rework_reason {
            writeln!(writer, "  reason: {reason}")?;
        }
    }
    writeln!(writer, "\nTask Conflict Hints:")?;
    if detail.conflict_hints.is_empty() {
        writeln!(writer, "(none)")?;
    } else {
        for hint in &detail.conflict_hints {
            writeln!(writer, "  {}. {}", hint.position, hint.target)?;
        }
    }
    writeln!(writer, "\nPotential Overlaps:")?;
    if detail.conflict_overlaps.is_empty() {
        writeln!(writer, "(none)")?;
    } else {
        for overlap in &detail.conflict_overlaps {
            writeln!(
                writer,
                "  {} -> {} [{}] {}",
                overlap.target, overlap.task_identifier, overlap.state, overlap.title
            )?;
        }
    }
    writeln!(writer, "\nWorkpad Note:")?;
    if let Some(note) = &detail.workpad_note {
        writeln!(writer, "{}", note.body)?;
    } else {
        writeln!(writer, "(none)")?;
    }
    Ok(())
}

pub fn write_queue(mut writer: impl Write, queue: &tasker_db::TaskQueue) -> Result<()> {
    writeln!(writer, "key: {}", queue.key)?;
    writeln!(writer, "name: {}", queue.name)?;
    writeln!(writer, "delivery backend: {}", queue.delivery_backend)?;
    writeln!(
        writer,
        "managed source repository: {}",
        queue.managed_source_repository
    )?;
    writeln!(writer, "main branch: {}", queue.main_branch)?;
    writeln!(writer, "worktree root: {}", queue.worktree_root)?;
    writeln!(writer, "branch template: {}", queue.branch_template)?;
    writeln!(
        writer,
        "done worktree retention: {}",
        queue.done_worktree_retention
    )?;
    match queue.queue_concurrency_limit {
        Some(limit) => writeln!(writer, "Queue Concurrency Limit: {limit}"),
        None => writeln!(writer, "Queue Concurrency Limit: none"),
    }
}

pub fn print_task_detail(detail: &tasker_db::TaskDetail) -> Result<()> {
    write_task_detail(std::io::stdout(), detail)
}

fn git_output(repo: &Path, args: &[&str]) -> Result<String> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn primary_task_link_target<'a>(links: &'a [tasker_db::TaskLink], kind: &str) -> Option<&'a str> {
    links
        .iter()
        .find(|link| link.kind == kind && link.is_primary)
        .or_else(|| links.iter().find(|link| link.kind == kind))
        .map(|link| link.target.as_str())
}

#[derive(Debug, Default, Serialize)]
pub struct TranscriptSummary {
    agent_end: Option<bool>,
    timed_out: Option<bool>,
    unattended_question_detected: Option<bool>,
    blocking_ui_request: Option<String>,
    exit_code: Option<i64>,
    stdout_bytes: Option<usize>,
    stderr_bytes: Option<usize>,
    warnings: Vec<String>,
}

impl TranscriptSummary {
    fn observe_event(&mut self, value: &serde_json::Value) {
        let type_name = value.get("type").and_then(|value| value.as_str());
        if type_name == Some("agent_end") {
            self.agent_end = Some(true);
        }
        if type_name == Some("extension_ui_request") {
            let method = value
                .get("method")
                .or_else(|| value.get("method_name"))
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            if method != "notify" {
                self.blocking_ui_request = Some(format!(
                    "blocking extension UI request {method} in unattended Worker Session"
                ));
            }
        }
        if value.get("event").and_then(|value| value.as_str()) == Some("question") {
            self.unattended_question_detected = Some(true);
        }
    }

    fn observe_transcript_record(&mut self, value: &serde_json::Value) {
        self.observe_event(value);
        if let Some(timed_out) = value.get("timed_out").and_then(|value| value.as_bool()) {
            self.timed_out = Some(timed_out);
        }
        if let Some(question) = value
            .get("unattended_question_detected")
            .and_then(|value| value.as_bool())
        {
            self.unattended_question_detected = Some(question);
        }
        if let Some(status) = value.get("status").and_then(|value| value.as_i64()) {
            self.exit_code = Some(status);
        }
        for field in ["stdout", "stderr"] {
            if let Some(text) = value.get(field).and_then(|value| value.as_str()) {
                if field == "stdout" {
                    self.stdout_bytes = Some(text.len());
                } else {
                    self.stderr_bytes = Some(text.len());
                }
                self.observe_embedded_json_lines(field, text);
            }
        }
    }

    fn observe_embedded_json_lines(&mut self, field: &str, text: &str) {
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

    fn observe_launcher_raw_json(&mut self, raw_json: Option<&str>) {
        let Some(raw_json) = raw_json else {
            return;
        };
        match serde_json::from_str::<serde_json::Value>(raw_json) {
            Ok(value) => {
                if let Some(timed_out) = value.get("timed_out").and_then(|value| value.as_bool()) {
                    self.timed_out = Some(timed_out);
                }
                if let Some(question) = value
                    .get("unattended_question_detected")
                    .and_then(|value| value.as_bool())
                {
                    self.unattended_question_detected = Some(question);
                }
                if let Some(exit_code) = value.get("exit_code").and_then(|value| value.as_i64()) {
                    self.exit_code = Some(exit_code);
                }
            }
            Err(error) => self.warnings.push(format!(
                "ignored malformed Launcher Session Data raw JSON: {error}"
            )),
        }
    }
}

fn summarize_transcript(path: &Path, raw_json: Option<&str>) -> TranscriptSummary {
    let mut summary = TranscriptSummary::default();
    summary.observe_launcher_raw_json(raw_json);
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            summary.warnings.push(format!(
                "could not read Run Transcript {}: {error}",
                path.display()
            ));
            return summary;
        }
    };

    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(value) => summary.observe_transcript_record(&value),
            Err(error) => summary.warnings.push(format!(
                "ignored malformed Run Transcript line {}: {error}",
                index + 1
            )),
        }
    }
    if summary.agent_end.is_none() {
        summary.agent_end = Some(false);
    }
    summary
}

fn yes_no_unknown(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

fn write_transcript_summary(
    mut writer: impl Write,
    path: &str,
    raw_json: Option<&str>,
) -> Result<()> {
    let summary = summarize_transcript(Path::new(path), raw_json);
    writeln!(writer, "\nRun Transcript Summary:")?;
    writeln!(writer, "  path: {path}")?;
    writeln!(
        writer,
        "  agent_end observed: {}",
        yes_no_unknown(summary.agent_end)
    )?;
    writeln!(writer, "  timed out: {}", yes_no_unknown(summary.timed_out))?;
    writeln!(
        writer,
        "  unattended question detected: {}",
        yes_no_unknown(summary.unattended_question_detected)
    )?;
    match &summary.blocking_ui_request {
        Some(reason) => writeln!(writer, "  blocking UI: {reason}")?,
        None => writeln!(writer, "  blocking UI: not detected")?,
    }
    if let Some(exit_code) = summary.exit_code {
        writeln!(writer, "  launcher exit code: {exit_code}")?;
    }
    if let Some(bytes) = summary.stdout_bytes {
        writeln!(writer, "  stdout captured: {bytes} bytes (content omitted)")?;
    }
    if let Some(bytes) = summary.stderr_bytes {
        writeln!(writer, "  stderr captured: {bytes} bytes (content omitted)")?;
    }
    for warning in summary.warnings {
        writeln!(writer, "  warning: {warning}")?;
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct LauncherSessionTelemetry<'a> {
    launcher_kind: &'a str,
    session_id: Option<&'a str>,
    model: Option<&'a str>,
    provider: Option<&'a str>,
    started_at: Option<&'a str>,
    finished_at: Option<&'a str>,
    final_status: Option<&'a str>,
    transcript_path: Option<&'a str>,
    transcript_summary: Option<TranscriptSummary>,
}

#[derive(Debug, Serialize)]
struct RunTelemetry<'a> {
    agent_run_id: &'a str,
    task_identifier: &'a str,
    task_title: &'a str,
    task_state: &'a str,
    worker_agent: &'a str,
    launcher: &'a str,
    claim_lease_expires_at: &'a str,
    outcome: Option<&'a str>,
    failure_reason: Option<&'a str>,
    failure_reason_code: Option<&'a str>,
    created_at: &'a str,
    finished_at: Option<&'a str>,
    launcher_session_data: Option<LauncherSessionTelemetry<'a>>,
    normalized_metrics: Option<&'a tasker_db::AgentRunMetrics>,
}

fn run_telemetry(detail: &tasker_db::AgentRunDetail) -> RunTelemetry<'_> {
    let launcher_session_data = detail.launcher_session_data.as_ref().map(|session| {
        let transcript_summary = session
            .transcript_path
            .as_deref()
            .map(|path| summarize_transcript(Path::new(path), session.raw_json.as_deref()));
        LauncherSessionTelemetry {
            launcher_kind: &session.launcher_kind,
            session_id: session.session_id.as_deref(),
            model: session.model.as_deref(),
            provider: session.provider.as_deref(),
            started_at: session.started_at.as_deref(),
            finished_at: session.finished_at.as_deref(),
            final_status: session.final_status.as_deref(),
            transcript_path: session.transcript_path.as_deref(),
            transcript_summary,
        }
    });
    RunTelemetry {
        agent_run_id: &detail.run.id,
        task_identifier: &detail.task.task.identifier,
        task_title: &detail.task.task.title,
        task_state: &detail.task.task.state,
        worker_agent: &detail.run.worker_actor_display_name,
        launcher: &detail.run.launcher_kind,
        claim_lease_expires_at: &detail.run.lease_expires_at,
        outcome: detail.run.outcome.as_deref(),
        failure_reason: detail.run.failure_reason.as_deref(),
        failure_reason_code: detail.run.failure_reason_code.as_deref(),
        created_at: &detail.run.created_at,
        finished_at: detail.run.finished_at.as_deref(),
        launcher_session_data,
        normalized_metrics: detail.metrics.as_ref(),
    }
}

pub fn write_run_detail_json(
    mut writer: impl Write,
    detail: &tasker_db::AgentRunDetail,
) -> Result<()> {
    serde_json::to_writer_pretty(&mut writer, &run_telemetry(detail))
        .map_err(std::io::Error::other)?;
    writeln!(writer)?;
    Ok(())
}

pub fn write_run_detail(mut writer: impl Write, detail: &tasker_db::AgentRunDetail) -> Result<()> {
    writeln!(writer, "Agent Run: {}", detail.run.id)?;
    writeln!(writer, "Task: {}", detail.task.task.identifier)?;
    writeln!(writer, "Task Title: {}", detail.task.task.title)?;
    writeln!(writer, "Task State: {}", detail.task.task.state)?;
    writeln!(
        writer,
        "Worker Agent: {}",
        detail.run.worker_actor_display_name
    )?;
    writeln!(writer, "Launcher: {}", detail.run.launcher_kind)?;
    writeln!(
        writer,
        "Claim Lease Expires At: {}",
        detail.run.lease_expires_at
    )?;
    writeln!(
        writer,
        "Outcome: {}",
        detail.run.outcome.as_deref().unwrap_or("active")
    )?;
    if let Some(code) = &detail.run.failure_reason_code {
        writeln!(writer, "Failure Reason Code: {code}")?;
    }
    if let Some(reason) = &detail.run.failure_reason {
        writeln!(writer, "Failure Reason: {reason}")?;
    }
    writeln!(writer, "Created At: {}", detail.run.created_at)?;
    if let Some(finished_at) = &detail.run.finished_at {
        writeln!(writer, "Finished At: {finished_at}")?;
    }
    writeln!(writer, "\nLauncher Session Data:")?;
    if let Some(session) = &detail.launcher_session_data {
        writeln!(writer, "  launcher kind: {}", session.launcher_kind)?;
        if let Some(session_id) = &session.session_id {
            writeln!(writer, "  session id: {session_id}")?;
        }
        if let Some(status) = &session.final_status {
            writeln!(writer, "  final status: {status}")?;
        }
        if let Some(path) = &session.transcript_path {
            writeln!(writer, "  Run Transcript: {path}")?;
            write_transcript_summary(&mut writer, path, session.raw_json.as_deref())?;
        }
    } else {
        writeln!(writer, "  (none)")?;
    }
    writeln!(writer, "\nNormalized Agent Run Metrics:")?;
    if let Some(metrics) = &detail.metrics {
        if let Some(duration_ms) = metrics.duration_ms {
            writeln!(writer, "  duration: {duration_ms} ms")?;
        }
        writeln!(writer, "  launcher kind: {}", metrics.launcher_kind)?;
        writeln!(
            writer,
            "  final status: {}",
            metrics.final_status.as_deref().unwrap_or("not recorded")
        )?;
        if let Some(exit_code) = metrics.exit_code {
            writeln!(writer, "  exit code: {exit_code}")?;
        }
        if let Some(timed_out) = metrics.timed_out {
            writeln!(writer, "  timed out: {}", timed_out != 0)?;
        }
        if let Some(question) = metrics.unattended_question_detected {
            writeln!(writer, "  unattended question detected: {}", question != 0)?;
        }
        if let Some(blocking_ui) = metrics.blocking_ui_detected {
            writeln!(writer, "  blocking UI detected: {}", blocking_ui != 0)?;
        }
        if let Some(path) = &metrics.transcript_path {
            writeln!(writer, "  transcript path: {path}")?;
        }
        if let Some(bytes) = metrics.transcript_byte_size {
            writeln!(writer, "  transcript byte size: {bytes}")?;
        }
        if let Some(count) = metrics.transcript_jsonl_event_count {
            writeln!(writer, "  JSONL event count: {count}")?;
        }
        if metrics.input_tokens.is_some()
            || metrics.output_tokens.is_some()
            || metrics.total_tokens.is_some()
            || metrics.cache_read_tokens.is_some()
            || metrics.cache_write_tokens.is_some()
        {
            writeln!(
                writer,
                "  tokens: input={} output={} total={} cache_read={} cache_write={}",
                metrics
                    .input_tokens
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                metrics
                    .output_tokens
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                metrics
                    .total_tokens
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                metrics
                    .cache_read_tokens
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                metrics
                    .cache_write_tokens
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            )?;
        }
        if metrics.tool_call_count.is_some()
            || metrics.tool_error_count.is_some()
            || metrics.assistant_turn_count.is_some()
            || metrics.user_turn_count.is_some()
        {
            writeln!(
                writer,
                "  efficiency: tool_calls={} tool_errors={} repeated_failed_tools={} repeated_reads={} repeated_tasker_context_fetches={} assistant_turns={} user_turns={} max_context_tokens={}",
                metrics.tool_call_count.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string()),
                metrics.tool_error_count.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string()),
                metrics.repeated_failed_tool_attempt_count.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string()),
                metrics.repeated_read_count.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string()),
                metrics.repeated_tasker_context_fetch_count.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string()),
                metrics.assistant_turn_count.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string()),
                metrics.user_turn_count.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string()),
                metrics.max_context_tokens.map(|value| value.to_string()).unwrap_or_else(|| "unknown".to_string())
            )?;
        }
        if metrics.tool_call_counts_json != "{}" {
            writeln!(
                writer,
                "  tool calls by tool: {}",
                metrics.tool_call_counts_json
            )?;
        }
        if metrics.shell_command_counts_json != "{}" {
            writeln!(
                writer,
                "  shell command categories: {}",
                metrics.shell_command_counts_json
            )?;
        }
        if let Ok(hints) = serde_json::from_str::<Vec<String>>(&metrics.efficiency_hints_json) {
            for hint in hints {
                writeln!(writer, "  optimization hint: {hint}")?;
            }
        }
        if let Ok(warnings) = serde_json::from_str::<Vec<String>>(&metrics.warnings_json) {
            for warning in warnings {
                writeln!(writer, "  warning: {warning}")?;
            }
        }
    } else {
        writeln!(writer, "  (not recorded)")?;
    }
    Ok(())
}

pub fn print_queue(queue: &tasker_db::TaskQueue) -> Result<()> {
    write_queue(std::io::stdout(), queue)
}

pub fn print_run_detail(detail: &tasker_db::AgentRunDetail) -> Result<()> {
    write_run_detail(std::io::stdout(), detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_run_detail(
        transcript_path: Option<String>,
        raw_json: Option<String>,
    ) -> tasker_db::AgentRunDetail {
        tasker_db::AgentRunDetail {
            run: tasker_db::AgentRun {
                id: "run-1".to_string(),
                task_id: "task-1".to_string(),
                task_queue_id: "queue-1".to_string(),
                worker_actor_kind: "worker_agent".to_string(),
                worker_actor_id: "worker-1".to_string(),
                worker_actor_display_name: "Worker 1".to_string(),
                worker_id: "worker-1".to_string(),
                launcher_kind: "pi".to_string(),
                lease_expires_at: "later".to_string(),
                last_heartbeat_at: None,
                outcome: Some("failed".to_string()),
                failure_reason: Some("launcher failure".to_string()),
                failure_reason_code: Some("launcher_exited".to_string()),
                created_at: "now".to_string(),
                finished_at: Some("later".to_string()),
            },
            task: tasker_db::TaskDetail {
                task: tasker_db::Task {
                    id: "task-1".to_string(),
                    task_queue_id: "queue-1".to_string(),
                    task_queue_key: "TASK".to_string(),
                    identifier: "TASK-1".to_string(),
                    sequence: 1,
                    title: "Test task".to_string(),
                    brief: "brief".to_string(),
                    priority: "normal".to_string(),
                    state: "in_progress".to_string(),
                    review_required: false,
                    validated_base_commit: None,
                    created_at: "now".to_string(),
                    updated_at: "now".to_string(),
                },
                acceptance_criteria: Vec::new(),
                validation_items: Vec::new(),
                tags: Vec::new(),
                workpad_note: None,
                task_links: Vec::new(),
                conflict_hints: Vec::new(),
                conflict_overlaps: Vec::new(),
                latest_rework_reason_code: None,
                latest_rework_reason: None,
            },
            launcher_session_data: Some(tasker_db::LauncherSessionData {
                agent_run_id: "run-1".to_string(),
                launcher_kind: "pi".to_string(),
                session_id: Some("session-1".to_string()),
                model: None,
                provider: None,
                started_at: Some("now".to_string()),
                finished_at: Some("later".to_string()),
                final_status: Some("failed".to_string()),
                transcript_path,
                raw_json,
                created_at: "now".to_string(),
                updated_at: "now".to_string(),
            }),
            metrics: None,
        }
    }

    #[test]
    fn queue_output_includes_concurrency_limit() {
        let queue = tasker_db::TaskQueue {
            id: "queue-id".to_string(),
            key: "TASK".to_string(),
            name: "Tasker".to_string(),
            delivery_backend: "local_worktree".to_string(),
            managed_source_repository: "/repo".to_string(),
            main_branch: "main".to_string(),
            worktree_root: "/worktrees".to_string(),
            branch_template: "tasker/{task_identifier}".to_string(),
            done_worktree_retention: false,
            queue_concurrency_limit: Some(1),
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        };
        let mut out = Vec::new();

        write_queue(&mut out, &queue).expect("write queue");
        let text = String::from_utf8(out).expect("utf8");

        assert!(text.contains("key: TASK"));
        assert!(text.contains("Queue Concurrency Limit: 1"));
    }

    #[test]
    fn run_detail_summarizes_transcript_signals_without_blobs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript_path = temp.path().join("pi.jsonl");
        std::fs::write(
            &transcript_path,
            serde_json::json!({
                "launcher": "pi",
                "status": 7,
                "stdout": "{\"type\":\"extension_ui_request\",\"method\":\"confirm\"}\n{\"type\":\"agent_end\"}\nsecret-token",
                "stderr": "very secret stderr",
                "timed_out": true,
                "unattended_question_detected": true
            })
            .to_string(),
        )
        .expect("write transcript");
        let detail = sample_run_detail(
            Some(transcript_path.display().to_string()),
            Some(serde_json::json!({"exit_code": 7, "timed_out": true}).to_string()),
        );
        let mut out = Vec::new();

        write_run_detail(&mut out, &detail).expect("write run");
        let text = String::from_utf8(out).expect("utf8");

        assert!(text.contains("Run Transcript Summary:"));
        assert!(text.contains("agent_end observed: yes"));
        assert!(text.contains("timed out: yes"));
        assert!(text.contains("unattended question detected: yes"));
        assert!(text.contains("blocking UI: blocking extension UI request confirm"));
        assert!(text.contains("launcher exit code: 7"));
        assert!(text.contains("stdout captured:"));
        assert!(text.contains("stderr captured:"));
        assert!(!text.contains("secret-token"));
        assert!(!text.contains("very secret stderr"));
    }

    #[test]
    fn run_detail_json_summarizes_transcript_without_raw_blobs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript_path = temp.path().join("pi.jsonl");
        std::fs::write(
            &transcript_path,
            serde_json::json!({
                "status": 7,
                "stdout": "{\"type\":\"agent_end\"}\nsecret-token",
                "stderr": "very secret stderr",
                "timed_out": true,
                "unattended_question_detected": false
            })
            .to_string(),
        )
        .expect("write transcript");
        let detail = sample_run_detail(
            Some(transcript_path.display().to_string()),
            Some(
                serde_json::json!({"exit_code": 7, "timed_out": true, "secret": "raw-secret"})
                    .to_string(),
            ),
        );
        let mut out = Vec::new();

        write_run_detail_json(&mut out, &detail).expect("write json");
        let text = String::from_utf8(out).expect("utf8");
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");

        assert_eq!(value["agent_run_id"], "run-1");
        assert_eq!(value["task_identifier"], "TASK-1");
        assert_eq!(
            value["launcher_session_data"]["transcript_summary"]["agent_end"],
            true
        );
        assert!(
            value["launcher_session_data"]["transcript_summary"]["stdout_bytes"]
                .as_u64()
                .expect("stdout bytes")
                > 0
        );
        assert!(!text.contains("secret-token"));
        assert!(!text.contains("very secret stderr"));
        assert!(!text.contains("raw-secret"));
    }

    #[test]
    fn run_detail_warns_for_missing_and_malformed_transcripts() {
        let missing_detail = sample_run_detail(Some("/no/such/transcript.jsonl".to_string()), None);
        let mut missing_out = Vec::new();
        write_run_detail(&mut missing_out, &missing_detail).expect("missing transcript warning");
        let missing_text = String::from_utf8(missing_out).expect("utf8");
        assert!(missing_text.contains("warning: could not read Run Transcript"));

        let temp = tempfile::tempdir().expect("tempdir");
        let transcript_path = temp.path().join("bad.jsonl");
        std::fs::write(&transcript_path, "not json\n{\"type\":\"agent_end\"}\n")
            .expect("write malformed transcript");
        let malformed_detail = sample_run_detail(Some(transcript_path.display().to_string()), None);
        let mut malformed_out = Vec::new();
        write_run_detail(&mut malformed_out, &malformed_detail)
            .expect("malformed transcript warning");
        let malformed_text = String::from_utf8(malformed_out).expect("utf8");
        assert!(malformed_text.contains("warning: ignored malformed Run Transcript line 1"));
        assert!(malformed_text.contains("agent_end observed: yes"));
    }
}

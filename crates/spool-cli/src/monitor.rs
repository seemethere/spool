use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    time::Duration,
};

use anyhow::{Context, Result};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers},
    execute, terminal,
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
    Frame, Terminal,
};
use sqlx::SqlitePool;

use crate::display;
use spool_runner::repo_lock;

const NEXT_TASK_LIMIT: usize = 5;

#[derive(Debug, Clone)]
pub struct MonitorOptions {
    pub queue: Option<String>,
    pub refresh_seconds: u64,
    pub plain: bool,
    pub once: bool,
    pub config_path: PathBuf,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorSnapshot {
    pub config_path: PathBuf,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub queue_filter: Option<String>,
    pub captured_at: String,
    pub queues: Vec<QueueSnapshot>,
    pub recent_runs: Vec<RecentRunSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSnapshot {
    pub key: String,
    pub name: String,
    pub state_counts: Vec<(String, i64)>,
    pub active_agent_runs: i64,
    pub active_retry_holds: i64,
    pub active_runs: Vec<spool_db::ActiveAgentRunStatus>,
    pub retry_holds: Vec<spool_db::ActiveRetryHoldStatus>,
    pub ready_tasks: Vec<spool_db::TaskStatusSummary>,
    pub rework_tasks: Vec<spool_db::TaskStatusSummary>,
    pub human_review_tasks: Vec<spool_db::TaskStatusSummary>,
    pub integrating_tasks: Vec<spool_db::TaskStatusSummary>,
    pub advisory_conflict_hints: Vec<spool_db::TaskConflictGroup>,
    pub integration_retries: Vec<spool_db::IntegrationRetryStatus>,
    pub repo_operation_lock: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RecentRunSnapshot {
    pub queue_key: String,
    pub task_identifier: String,
    pub task_title: String,
    pub agent_run_id: String,
    pub launcher_kind: String,
    pub worker_id: String,
    pub outcome: Option<String>,
    pub failure_reason: Option<String>,
    pub failure_reason_code: Option<String>,
    pub task_state: String,
    pub recovered_by_later_success: bool,
    pub created_at: String,
    pub finished_at: Option<String>,
}

pub async fn run_monitor(pool: &SqlitePool, options: MonitorOptions) -> Result<()> {
    if options.plain || options.once || !stdout_supports_terminal_ui() {
        let snapshot = load_snapshot(pool, &options).await?;
        write_snapshot(io::stdout(), &snapshot)?;
        if !options.once && !options.plain && !stdout_supports_terminal_ui() {
            eprintln!(
                "spool monitor: stdout is not an interactive terminal; printed one plain snapshot"
            );
        }
        return Ok(());
    }

    let mut stdout = io::stdout();
    terminal::enable_raw_mode().context("failed to enable terminal raw mode")?;
    if let Err(error) = execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide) {
        let _ = terminal::disable_raw_mode();
        return Err(error).context("failed to enter terminal monitor");
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("failed to initialize terminal monitor");
        }
    };
    let result = run_terminal_loop(pool, &options, &mut terminal).await;
    let show_cursor = terminal
        .show_cursor()
        .context("failed to show terminal cursor");
    let leave_screen = execute!(terminal.backend_mut(), terminal::LeaveAlternateScreen)
        .context("failed to leave terminal monitor");
    let disable_raw = terminal::disable_raw_mode().context("failed to disable terminal raw mode");
    result.and(show_cursor).and(leave_screen).and(disable_raw)
}

async fn run_terminal_loop<B: Backend>(
    pool: &SqlitePool,
    options: &MonitorOptions,
    terminal: &mut Terminal<B>,
) -> Result<()> {
    let refresh = Duration::from_secs(options.refresh_seconds.max(1));
    loop {
        let snapshot = load_snapshot(pool, options).await?;
        terminal
            .draw(|frame| render_snapshot(frame, &snapshot))
            .context("failed to redraw terminal monitor")?;

        let deadline = std::time::Instant::now() + refresh;
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            if event::poll(deadline.saturating_duration_since(now))? {
                match event::read()? {
                    Event::Key(key)
                        if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                            || (matches!(key.code, KeyCode::Char('c'))
                                && key.modifiers.contains(KeyModifiers::CONTROL)) =>
                    {
                        return Ok(())
                    }
                    Event::Key(key) if matches!(key.code, KeyCode::Char('r')) => break,
                    _ => {}
                }
            }
        }
    }
}

fn render_snapshot(frame: &mut Frame<'_>, snapshot: &MonitorSnapshot) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Percentage(42),
            Constraint::Percentage(38),
            Constraint::Length(3),
        ])
        .split(frame.area());

    render_header(frame, chunks[0], snapshot);
    render_attention(frame, chunks[1], snapshot);
    render_work_board(frame, chunks[2], snapshot);
    render_footer(frame, chunks[3]);
}

fn render_header(frame: &mut Frame<'_>, area: Rect, snapshot: &MonitorSnapshot) {
    let queue = snapshot.queue_filter.as_deref().unwrap_or("all");
    let attention_count = attention_lines(snapshot).len();
    let running_count: usize = snapshot
        .queues
        .iter()
        .map(|queue| queue.active_runs.len())
        .sum();
    let next_count: usize = snapshot
        .queues
        .iter()
        .map(|queue| queue.ready_tasks.len().min(NEXT_TASK_LIMIT))
        .sum();
    let lines = vec![
        Line::from(vec![
            Span::styled("◆ Spool", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw(" attention board"),
        ]),
        Line::from(format!(
            "needs attention: {attention_count} | running: {running_count} | next shown: {next_count} | queue: {queue}"
        )),
        Line::from(format!("captured: {}", snapshot.captured_at)),
        Line::from(format!("config: {}", snapshot.config_path.display())),
        Line::from(format!("data: {} | db: {}", snapshot.data_dir.display(), snapshot.db_path.display())),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title("Status").borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_attention(frame: &mut Frame<'_>, area: Rect, snapshot: &MonitorSnapshot) {
    let mut lines = attention_lines(snapshot);
    if lines.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("✓", Style::default().fg(Color::Green)),
            Span::raw(" no operator attention needed"),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title("Needs Attention")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_work_board(frame: &mut Frame<'_>, area: Rect, snapshot: &MonitorSnapshot) {
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(46), Constraint::Percentage(54)])
        .split(panes[0]);

    let mut running_lines = Vec::new();
    for queue in &snapshot.queues {
        for run in &queue.active_runs {
            if is_stale_lease(&run.lease_expires_at, &snapshot.captured_at) {
                continue;
            }
            running_lines.push(Line::from(vec![
                Span::styled("● ", Style::default().fg(Color::Green)),
                Span::styled(
                    compact_task_label(&run.task_identifier, &run.task_title, 38),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    "  {}  {}",
                    running_state_label(&run.task_state),
                    compact_run_id(&run.agent_run_id)
                )),
            ]));
        }
    }
    if running_lines.is_empty() {
        running_lines.push(Line::from("No healthy active Agent Runs"));
    }
    frame.render_widget(
        Paragraph::new(running_lines)
            .block(Block::default().title("Running").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        left[0],
    );

    frame.render_widget(
        Paragraph::new(rework_lines(snapshot, 38))
            .block(Block::default().title("Rework").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        left[1],
    );

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(28),
            Constraint::Percentage(38),
        ])
        .split(panes[1]);

    let mut next_lines = Vec::new();
    for queue in &snapshot.queues {
        for task in queue.ready_tasks.iter().take(NEXT_TASK_LIMIT) {
            let blocked_by = task
                .blocking_task_identifiers
                .as_deref()
                .filter(|_| task.unresolved_blocking_task_count > 0)
                .map(|blocking| format!("  blocked_by={blocking}"))
                .unwrap_or_default();
            next_lines.push(Line::from(format!(
                "› {}  {}{}",
                compact_task_label(&task.identifier, &task.title, 34),
                task.priority,
                blocked_by
            )));
        }
        if queue.ready_tasks.len() > NEXT_TASK_LIMIT {
            next_lines.push(Line::from(format!(
                "… {} more Ready Tasks in {} (use spool status)",
                queue.ready_tasks.len() - NEXT_TASK_LIMIT,
                queue.key
            )));
        }
    }
    if next_lines.is_empty() {
        next_lines.push(Line::from("No Ready Tasks waiting"));
    }
    frame.render_widget(
        Paragraph::new(next_lines)
            .block(Block::default().title("Next").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        right[0],
    );

    frame.render_widget(
        Paragraph::new(advisory_conflict_lines(snapshot))
            .block(
                Block::default()
                    .title("Advisory Task Conflict Hints")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        right[1],
    );

    render_recent_runs(frame, right[2], snapshot);
}

fn render_recent_runs(frame: &mut Frame<'_>, area: Rect, snapshot: &MonitorSnapshot) {
    let recent_rows = snapshot.recent_runs.iter().take(5).map(|run| {
        Row::new(vec![
            Cell::from(compact_task_label(
                &run.task_identifier,
                &run.task_title,
                24,
            )),
            Cell::from(run.outcome.clone().unwrap_or_else(|| "active".to_string())),
            Cell::from(run.finished_at.clone().unwrap_or_else(|| "-".to_string())),
        ])
    });
    let table = Table::new(
        recent_rows,
        [
            Constraint::Length(26),
            Constraint::Length(10),
            Constraint::Min(8),
        ],
    )
    .header(
        Row::new(["Task", "Outcome", "Finished"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .title("Recent Agent Runs")
            .borders(Borders::ALL),
    );
    frame.render_widget(table, area);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(
            "q quit  r refresh  ? help: use `spool monitor --help` or `spool status` for detail",
        )
        .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn rework_lines(snapshot: &MonitorSnapshot, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for queue in &snapshot.queues {
        for task in &queue.rework_tasks {
            let progress = if has_healthy_active_run(queue, &task.identifier, &snapshot.captured_at)
            {
                " — rework in progress"
            } else {
                ""
            };
            lines.push(Line::from(format!(
                "↻ {}  code={}{}",
                compact_task_label(&task.identifier, &task.title, width),
                task.latest_rework_reason_code
                    .as_deref()
                    .unwrap_or("unknown_legacy"),
                progress
            )));
            lines.push(Line::from(format!(
                "  local_worktree={}",
                local_worktree_status(
                    task.local_worktree.as_deref(),
                    task.task_branch.as_deref(),
                    &task.main_branch,
                )
            )));
            if let Some(reason) = &task.latest_rework_reason {
                lines.push(Line::from(format!("  reason: {reason}")));
            }
        }
    }
    if lines.is_empty() {
        lines.push(Line::from("No Rework Tasks"));
    }
    lines
}

fn advisory_conflict_lines(snapshot: &MonitorSnapshot) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for queue in &snapshot.queues {
        for group in &queue.advisory_conflict_hints {
            lines.push(Line::from(format!(
                "{}: {} Task(s) — {}",
                group.target, group.task_count, group.tasks
            )));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from("No advisory conflict hints"));
    }
    lines
}

fn running_state_label(task_state: &str) -> &str {
    if task_state == "rework" {
        "rework in progress"
    } else {
        task_state
    }
}

fn has_healthy_active_run(queue: &QueueSnapshot, task_identifier: &str, captured_at: &str) -> bool {
    queue.active_runs.iter().any(|run| {
        run.task_identifier == task_identifier
            && !is_stale_lease(&run.lease_expires_at, captured_at)
    })
}

fn has_healthy_active_run_for_snapshot(snapshot: &MonitorSnapshot, task_identifier: &str) -> bool {
    snapshot
        .queues
        .iter()
        .any(|queue| has_healthy_active_run(queue, task_identifier, &snapshot.captured_at))
}

fn attention_lines(snapshot: &MonitorSnapshot) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for queue in &snapshot.queues {
        if let Some(lock) = &queue.repo_operation_lock {
            lines.push(attention_line("⛔", "repo lock", lock.clone()));
        }
        for run in &queue.active_runs {
            if is_stale_lease(&run.lease_expires_at, &snapshot.captured_at) {
                lines.push(attention_line(
                    "⚠",
                    "stale run",
                    format!(
                        "{} lease expired {} worker={}",
                        compact_task_label(&run.task_identifier, &run.task_title, 44),
                        run.lease_expires_at,
                        run.worker_id
                    ),
                ));
            }
        }
        for task in &queue.rework_tasks {
            if has_healthy_active_run(queue, &task.identifier, &snapshot.captured_at) {
                continue;
            }
            lines.push(attention_line(
                "↻",
                "rework",
                format!(
                    "{} waiting for Worker Agent/operator; code={}",
                    compact_task_label(&task.identifier, &task.title, 44),
                    task.latest_rework_reason_code
                        .as_deref()
                        .unwrap_or("unknown_legacy")
                ),
            ));
        }
        for task in &queue.human_review_tasks {
            lines.push(attention_line(
                "☞",
                "human review",
                human_review_attention_detail(queue, task, 44),
            ));
        }
        for hold in &queue.retry_holds {
            lines.push(attention_line(
                "⏸",
                "retry hold",
                format!(
                    "{} until {} — {}",
                    hold.task_identifier, hold.hold_until, hold.reason
                ),
            ));
        }
        for retry in &queue.integration_retries {
            lines.push(attention_line(
                "↻",
                "integration",
                format!(
                    "{} code={} attempt={} next={} hint={}{}",
                    compact_task_label(&retry.task_identifier, &retry.task_title, 44),
                    retry.reason_code,
                    retry.retry_attempt.unwrap_or_default(),
                    retry.next_retry_at.as_deref().unwrap_or("operator"),
                    integration_recovery_hint(&retry.reason_code),
                    retry
                        .reason
                        .as_deref()
                        .map(|r| format!(" — {r}"))
                        .unwrap_or_default()
                ),
            ));
        }
        for task in &queue.integrating_tasks {
            if !queue
                .active_runs
                .iter()
                .any(|run| run.task_identifier == task.identifier)
            {
                lines.push(attention_line(
                    "◈",
                    "integrating",
                    format!(
                        "{} waiting for delivery progress",
                        compact_task_label(&task.identifier, &task.title, 44)
                    ),
                ));
            }
        }
    }
    for run in &snapshot.recent_runs {
        if is_unrecovered_attention_outcome(run)
            && !has_healthy_active_run_for_snapshot(snapshot, &run.task_identifier)
        {
            lines.push(attention_line(
                "✖",
                "failed run",
                format!(
                    "{} {}{}",
                    compact_task_label(&run.task_identifier, &run.task_title, 44),
                    run.outcome.as_deref().unwrap_or("failed"),
                    run.failure_reason
                        .as_deref()
                        .map(|reason| format!(" — {reason}"))
                        .unwrap_or_default()
                ),
            ));
        }
    }
    lines
}

fn attention_texts(snapshot: &MonitorSnapshot) -> Vec<String> {
    let mut lines = Vec::new();
    for queue in &snapshot.queues {
        if let Some(lock) = &queue.repo_operation_lock {
            lines.push(format!("repo lock: {lock}"));
        }
        for run in &queue.active_runs {
            if is_stale_lease(&run.lease_expires_at, &snapshot.captured_at) {
                lines.push(format!(
                    "stale run: {} lease expired {} worker={}",
                    compact_task_label(&run.task_identifier, &run.task_title, 64),
                    run.lease_expires_at,
                    run.worker_id
                ));
            }
        }
        for task in &queue.rework_tasks {
            if has_healthy_active_run(queue, &task.identifier, &snapshot.captured_at) {
                continue;
            }
            lines.push(format!(
                "rework: {} waiting for Worker Agent/operator; code={}",
                compact_task_label(&task.identifier, &task.title, 64),
                task.latest_rework_reason_code
                    .as_deref()
                    .unwrap_or("unknown_legacy")
            ));
        }
        for task in &queue.human_review_tasks {
            lines.push(format!(
                "human review: {}",
                human_review_attention_detail(queue, task, 64)
            ));
        }
        for hold in &queue.retry_holds {
            lines.push(format!(
                "retry hold: {} until {} — {}",
                hold.task_identifier, hold.hold_until, hold.reason
            ));
        }
        for retry in &queue.integration_retries {
            lines.push(format!(
                "integration: {} code={} attempt={} next={} hint={}{}",
                compact_task_label(&retry.task_identifier, &retry.task_title, 64),
                retry.reason_code,
                retry.retry_attempt.unwrap_or_default(),
                retry.next_retry_at.as_deref().unwrap_or("operator"),
                integration_recovery_hint(&retry.reason_code),
                retry
                    .reason
                    .as_deref()
                    .map(|r| format!(" — {r}"))
                    .unwrap_or_default()
            ));
        }
        for task in &queue.integrating_tasks {
            if !queue
                .active_runs
                .iter()
                .any(|run| run.task_identifier == task.identifier)
            {
                lines.push(format!(
                    "integrating: {} waiting for delivery progress",
                    compact_task_label(&task.identifier, &task.title, 64)
                ));
            }
        }
    }
    for run in &snapshot.recent_runs {
        if is_unrecovered_attention_outcome(run)
            && !has_healthy_active_run_for_snapshot(snapshot, &run.task_identifier)
        {
            lines.push(format!(
                "failed run: {} {}{}",
                compact_task_label(&run.task_identifier, &run.task_title, 64),
                run.outcome.as_deref().unwrap_or("failed"),
                run.failure_reason
                    .as_deref()
                    .map(|reason| format!(" — {reason}"))
                    .unwrap_or_default()
            ));
        }
    }
    lines
}

fn human_review_attention_detail(
    queue: &QueueSnapshot,
    task: &spool_db::TaskStatusSummary,
    width: usize,
) -> String {
    let mut detail = format!(
        "{} waiting for Review Decision",
        compact_task_label(&task.identifier, &task.title, width)
    );
    let blocked_ready = ready_tasks_blocked_by(queue, &task.identifier, width);
    if !blocked_ready.is_empty() {
        detail.push_str("; blocks Ready Tasks: ");
        detail.push_str(&blocked_ready.join(", "));
    }
    detail
}

fn ready_tasks_blocked_by(
    queue: &QueueSnapshot,
    blocking_identifier: &str,
    width: usize,
) -> Vec<String> {
    queue
        .ready_tasks
        .iter()
        .filter(|task| task.unresolved_blocking_task_count > 0)
        .filter(|task| {
            task.blocking_task_identifiers
                .as_deref()
                .is_some_and(|blocking_tasks| {
                    blocking_tasks.split(',').any(|entry| {
                        let identifier = entry
                            .trim()
                            .split_once(':')
                            .map(|(identifier, _)| identifier.trim())
                            .unwrap_or_else(|| entry.trim());
                        identifier == blocking_identifier
                    })
                })
        })
        .map(|task| compact_task_label(&task.identifier, &task.title, width))
        .collect()
}

fn local_worktree_status(
    local_worktree: Option<&str>,
    task_branch: Option<&str>,
    main_branch: &str,
) -> String {
    let Some(path) = local_worktree else {
        return "missing Task Link".to_string();
    };
    let worktree = Path::new(path);
    if !worktree.exists() {
        return format!("missing ({path})");
    }

    let cleanliness = match git_output(worktree, &["status", "--porcelain"]) {
        Ok(status) if status.trim().is_empty() => "clean".to_string(),
        Ok(_) => "dirty".to_string(),
        Err(_) => "exists, git status unavailable".to_string(),
    };
    let branch = git_output(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .map(|branch| branch.trim().to_string());
    let branch_note = match (branch.as_deref(), task_branch) {
        (Some(actual), Some(expected)) if actual != expected => {
            format!(" branch={actual} expected={expected}")
        }
        (Some(actual), _) => format!(" branch={actual}"),
        (None, Some(expected)) => format!(" branch=unknown expected={expected}"),
        (None, None) => " branch=unknown".to_string(),
    };
    let includes_main = match git_status(
        worktree,
        &["merge-base", "--is-ancestor", main_branch, "HEAD"],
    ) {
        Ok(status) if status.success() => "yes",
        Ok(_) => "no",
        Err(_) => "unknown",
    };

    format!("{cleanliness}{branch_note} includes_main={includes_main} ({path})")
}

fn git_output(repo: &Path, args: &[&str]) -> Result<String> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {:?} in {}", args, repo.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_status(repo: &Path, args: &[&str]) -> Result<std::process::ExitStatus> {
    ProcessCommand::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .with_context(|| format!("failed to run git {:?} in {}", args, repo.display()))
}

fn integration_recovery_hint(reason_code: &str) -> &'static str {
    match reason_code {
        "dirty_managed_source_repository" => "clean managed repo",
        "repo_operation_lock_held" => "wait/release repo lock",
        "uncommitted_local_worktree" => "commit/discard worktree changes",
        "stale_validated_base_commit" => "rebase or revalidate",
        "auto_refresh_conflict" => "resolve auto-refresh conflicts",
        "auto_refresh_validation_failed" => "fix auto-refresh validation",
        "auto_refresh_declined_missing_validation" => "configure validation commands",
        "task_branch_missing_main" => "update Task Branch from Main",
        "merge_conflict" => "resolve conflicts in Rework",
        "cleanup_failure" => "manual cleanup",
        "unknown_legacy" => "inspect legacy message",
        _ => "inspect outcome message",
    }
}

fn attention_line(icon: &'static str, label: &'static str, detail: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{icon} "), Style::default().fg(Color::Yellow)),
        Span::styled(
            format!("{label}: "),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(detail),
    ])
}

fn is_unrecovered_attention_outcome(run: &RecentRunSnapshot) -> bool {
    matches!(
        run.outcome.as_deref(),
        Some("failed" | "expired" | "canceled")
    ) && !is_terminal_task_state(&run.task_state)
        && !run.recovered_by_later_success
}

fn is_terminal_task_state(task_state: &str) -> bool {
    matches!(task_state, "done" | "canceled")
}

fn is_stale_lease(lease_expires_at: &str, captured_at: &str) -> bool {
    lease_expires_at <= captured_at
}

fn compact_task_label(identifier: &str, title: &str, width: usize) -> String {
    display::task_label(identifier, title, width)
}

fn compact_run_id(run_id: &str) -> String {
    run_id.chars().take(8).collect()
}

#[cfg(test)]
struct CrLfWriter<W> {
    inner: W,
}

#[cfg(test)]
impl<W> CrLfWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner }
    }
}

#[cfg(test)]
impl<W: Write> Write for CrLfWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for byte in buf {
            if *byte == b'\n' {
                self.inner.write_all(b"\r\n")?;
            } else {
                self.inner.write_all(&[*byte])?;
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn stdout_supports_terminal_ui() -> bool {
    terminal_supports_ui(
        io::stdout().is_terminal(),
        std::env::var("TERM").ok().as_deref(),
    )
}

fn terminal_supports_ui(is_terminal: bool, term: Option<&str>) -> bool {
    is_terminal && !matches!(term, Some("dumb"))
}

pub async fn load_snapshot(pool: &SqlitePool, options: &MonitorOptions) -> Result<MonitorSnapshot> {
    let mut rows = spool_db::status_by_queue_and_state(pool).await?;
    let mut active_runs = spool_db::active_agent_runs_for_status(pool).await?;
    let mut retry_holds = spool_db::active_retry_holds_for_status(pool).await?;
    let mut status_tasks = spool_db::tasks_for_status_by_states(
        pool,
        &["ready", "human_review", "rework", "integrating"],
    )
    .await?;
    let mut conflict_groups = spool_db::task_conflict_groups_for_status(pool).await?;
    let recent_runs = recent_agent_runs(pool, options.queue.as_deref()).await?;
    let mut integration_retries = spool_db::integration_retries_for_status(pool).await?;

    if let Some(queue) = &options.queue {
        rows.retain(|row| row.queue_key == *queue);
        active_runs.retain(|run| run.queue_key == *queue);
        retry_holds.retain(|hold| hold.queue_key == *queue);
        status_tasks.retain(|task| task.queue_key == *queue);
        conflict_groups.retain(|group| group.queue_key == *queue);
        integration_retries.retain(|retry| retry.queue_key == *queue);
    }

    let captured_at: String = sqlx::query_scalar("SELECT CURRENT_TIMESTAMP")
        .fetch_one(pool)
        .await
        .context("failed to capture monitor timestamp")?;

    let mut by_queue: BTreeMap<String, QueueSnapshot> = BTreeMap::new();
    for row in rows {
        let queue = by_queue
            .entry(row.queue_key.clone())
            .or_insert_with(|| QueueSnapshot {
                key: row.queue_key.clone(),
                name: row.queue_name.clone(),
                state_counts: Vec::new(),
                active_agent_runs: row.active_agent_runs,
                active_retry_holds: row.active_retry_holds,
                active_runs: Vec::new(),
                retry_holds: Vec::new(),
                ready_tasks: Vec::new(),
                rework_tasks: Vec::new(),
                human_review_tasks: Vec::new(),
                integrating_tasks: Vec::new(),
                advisory_conflict_hints: Vec::new(),
                integration_retries: Vec::new(),
                repo_operation_lock: None,
            });
        queue.state_counts.push((row.state, row.task_count));
        queue.active_agent_runs = row.active_agent_runs;
        queue.active_retry_holds = row.active_retry_holds;
    }
    for run in active_runs {
        if let Some(queue) = by_queue.get_mut(&run.queue_key) {
            queue.active_runs.push(run);
        }
    }
    for hold in retry_holds {
        if let Some(queue) = by_queue.get_mut(&hold.queue_key) {
            queue.retry_holds.push(hold);
        }
    }
    for task in status_tasks {
        if let Some(queue) = by_queue.get_mut(&task.queue_key) {
            match task.state.as_str() {
                "ready" => queue.ready_tasks.push(task),
                "human_review" => queue.human_review_tasks.push(task),
                "rework" => queue.rework_tasks.push(task),
                "integrating" => queue.integrating_tasks.push(task),
                _ => {}
            }
        }
    }
    for group in conflict_groups {
        if let Some(queue) = by_queue.get_mut(&group.queue_key) {
            queue.advisory_conflict_hints.push(group);
        }
    }
    for retry in integration_retries {
        if let Some(queue) = by_queue.get_mut(&retry.queue_key) {
            queue.integration_retries.push(retry);
        }
    }

    for queue in by_queue.values_mut() {
        if let Some(active) = repo_lock::active_lock(&options.data_dir, &queue.key)? {
            queue.repo_operation_lock = Some(format!(
                "{} pid={} operation={}{}",
                queue.key,
                active.lock.pid,
                active.lock.operation,
                active
                    .lock
                    .task_identifier
                    .as_deref()
                    .map(|task| format!(" task={task}"))
                    .unwrap_or_default()
            ));
        }
    }

    Ok(MonitorSnapshot {
        config_path: options.config_path.clone(),
        data_dir: options.data_dir.clone(),
        db_path: options.db_path.clone(),
        queue_filter: options.queue.clone(),
        captured_at,
        queues: by_queue.into_values().collect(),
        recent_runs,
    })
}

async fn recent_agent_runs(
    pool: &SqlitePool,
    queue_filter: Option<&str>,
) -> Result<Vec<RecentRunSnapshot>> {
    let mut query = sqlx::QueryBuilder::new(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS task_identifier,
            tasks.title AS task_title,
            agent_runs.id AS agent_run_id,
            agent_runs.launcher_kind AS launcher_kind,
            agent_runs.worker_id AS worker_id,
            agent_runs.outcome AS outcome,
            agent_runs.failure_reason AS failure_reason,
            agent_runs.failure_reason_code AS failure_reason_code,
            tasks.state AS task_state,
            EXISTS (
                SELECT 1
                FROM agent_runs AS later_runs
                WHERE later_runs.task_id = agent_runs.task_id
                  AND later_runs.outcome = 'completed'
                  AND (
                      later_runs.created_at > agent_runs.created_at
                      OR (
                          later_runs.created_at = agent_runs.created_at
                          AND later_runs.rowid > agent_runs.rowid
                      )
                  )
            ) AS recovered_by_later_success,
            agent_runs.created_at AS created_at,
            agent_runs.finished_at AS finished_at
        FROM agent_runs
        JOIN tasks ON tasks.id = agent_runs.task_id
        JOIN task_queues ON task_queues.id = agent_runs.task_queue_id
        "#,
    );
    if let Some(queue) = queue_filter {
        query.push(" WHERE task_queues.key = ").push_bind(queue);
    }
    query.push(" ORDER BY agent_runs.created_at DESC, agent_runs.rowid DESC LIMIT 10");
    query
        .build_query_as::<RecentRunSnapshot>()
        .fetch_all(pool)
        .await
        .context("failed to load recent Agent Runs")
}

pub fn write_snapshot(mut writer: impl Write, snapshot: &MonitorSnapshot) -> io::Result<()> {
    writeln!(writer, "Spool attention board")?;
    writeln!(writer, "captured at: {}", snapshot.captured_at)?;
    writeln!(writer, "config: {}", snapshot.config_path.display())?;
    writeln!(writer, "data: {}", snapshot.data_dir.display())?;
    writeln!(writer, "database: {}", snapshot.db_path.display())?;
    if let Some(queue) = &snapshot.queue_filter {
        writeln!(writer, "queue filter: {queue}")?;
    }
    writeln!(
        writer,
        "keys: q quit, r refresh; use spool status for details"
    )?;

    if snapshot.queues.is_empty() {
        writeln!(writer, "\nNo Task Queues found")?;
    }

    writeln!(writer, "\nNeeds Attention:")?;
    let attention = attention_texts(snapshot);
    if attention.is_empty() {
        writeln!(writer, "  ✓ none")?;
    } else {
        for item in attention {
            writeln!(writer, "  {item}")?;
        }
    }

    writeln!(writer, "\nRunning:")?;
    let mut running = 0;
    for queue in &snapshot.queues {
        for run in &queue.active_runs {
            if is_stale_lease(&run.lease_expires_at, &snapshot.captured_at) {
                continue;
            }
            running += 1;
            writeln!(
                writer,
                "  ● {}\tstate={}\trun={}\tworker={}",
                compact_task_label(&run.task_identifier, &run.task_title, 64),
                running_state_label(&run.task_state),
                compact_run_id(&run.agent_run_id),
                run.worker_id
            )?;
        }
    }
    if running == 0 {
        writeln!(writer, "  (none)")?;
    }

    writeln!(writer, "\nRework:")?;
    let mut rework_count = 0;
    for queue in &snapshot.queues {
        for task in &queue.rework_tasks {
            rework_count += 1;
            writeln!(
                writer,
                "  ↻ {}\treason_code={}\tlocal_worktree={}",
                compact_task_label(&task.identifier, &task.title, 64),
                task.latest_rework_reason_code
                    .as_deref()
                    .unwrap_or("unknown_legacy"),
                local_worktree_status(
                    task.local_worktree.as_deref(),
                    task.task_branch.as_deref(),
                    &task.main_branch,
                )
            )?;
            if let Some(reason) = &task.latest_rework_reason {
                writeln!(writer, "    reason: {reason}")?;
            }
        }
    }
    if rework_count == 0 {
        writeln!(writer, "  (none)")?;
    }

    writeln!(writer, "\nHuman Review:")?;
    let mut human_review_count = 0;
    for queue in &snapshot.queues {
        for task in &queue.human_review_tasks {
            human_review_count += 1;
            writeln!(
                writer,
                "  ☞ {}",
                human_review_attention_detail(queue, task, 64)
            )?;
        }
    }
    if human_review_count == 0 {
        writeln!(writer, "  (none)")?;
    }

    writeln!(writer, "\nNext:")?;
    let mut next = 0;
    for queue in &snapshot.queues {
        for task in queue.ready_tasks.iter().take(NEXT_TASK_LIMIT) {
            next += 1;
            if task.unresolved_blocking_task_count > 0 {
                writeln!(
                    writer,
                    "  › {}\tpriority={}\tblocked_by={}",
                    compact_task_label(&task.identifier, &task.title, 64),
                    task.priority,
                    task.blocking_task_identifiers.as_deref().unwrap_or("")
                )?;
            } else {
                writeln!(
                    writer,
                    "  › {}\tpriority={}",
                    compact_task_label(&task.identifier, &task.title, 64),
                    task.priority
                )?;
            }
        }
        if queue.ready_tasks.len() > NEXT_TASK_LIMIT {
            writeln!(
                writer,
                "  … {} more Ready Tasks in {} (use spool status)",
                queue.ready_tasks.len() - NEXT_TASK_LIMIT,
                queue.key
            )?;
        }
    }
    if next == 0 {
        writeln!(writer, "  (none)")?;
    }

    writeln!(writer, "\nAdvisory Task Conflict Hints:")?;
    let mut conflict_count = 0;
    for queue in &snapshot.queues {
        for group in &queue.advisory_conflict_hints {
            conflict_count += 1;
            writeln!(
                writer,
                "  {}\t{} Task(s): {}",
                group.target, group.task_count, group.tasks
            )?;
        }
    }
    if conflict_count == 0 {
        writeln!(writer, "  (none)")?;
    }

    writeln!(writer, "\nRecent Agent Runs:")?;
    if snapshot.recent_runs.is_empty() {
        writeln!(writer, "  (none)")?;
    }
    for run in snapshot.recent_runs.iter().take(5) {
        let status = run.outcome.as_deref().unwrap_or("active");
        writeln!(
            writer,
            "  {}\t{}\t{}\tfinished={}",
            run.queue_key,
            compact_task_label(&run.task_identifier, &run.task_title, 64),
            status,
            run.finished_at.as_deref().unwrap_or("-")
        )?;
        if let Some(code) = &run.failure_reason_code {
            writeln!(writer, "    failure code: {code}")?;
        }
        if let Some(reason) = &run.failure_reason {
            writeln!(writer, "    failure reason: {reason}")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn run_git(repo: &Path, args: &[&str]) {
        let status = ProcessCommand::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git {:?} failed", args);
    }

    async fn temp_pool() -> SqlitePool {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("spool.db");
        // Keep the tempdir alive by leaking it for the duration of the test process.
        let _temp = Box::leak(Box::new(temp));
        let pool = spool_db::connect(&db_path).await.expect("connect db");
        spool_db::run_migrations(&pool).await.expect("migrations");
        pool
    }

    async fn seed_queue_and_task(pool: &SqlitePool) -> String {
        spool_db::create_task_queue(
            pool,
            &spool_db::CreateTaskQueue {
                key: "TASK".to_string(),
                name: "Spool".to_string(),
                managed_source_repository: "/repo".to_string(),
                main_branch: "main".to_string(),
                worktree_root: "/worktrees".to_string(),
                branch_template: "spool/{task_identifier}".to_string(),
                done_worktree_retention: false,
                queue_concurrency_limit: Some(1),
            },
            &spool_db::Actor::operator("tester"),
        )
        .await
        .expect("create queue");
        let task = spool_db::create_task(
            pool,
            &spool_db::CreateTask {
                queue_key: "TASK".to_string(),
                title: "Do work".to_string(),
                brief: "brief".to_string(),
                priority: "normal".to_string(),
                state: "ready".to_string(),
                review_required: false,
                acceptance_criteria: vec!["criterion".to_string()],
                validation_items: vec!["validation".to_string()],
                tags: Vec::new(),
                conflict_hints: Vec::new(),
                blocking_task_identifiers: Vec::new(),
            },
            &spool_db::Actor::operator("tester"),
        )
        .await
        .expect("create task");
        task.task.identifier
    }

    fn worker_actor(id: &str) -> spool_db::Actor {
        spool_db::Actor {
            kind: "worker_agent".to_string(),
            id: id.to_string(),
            display_name: id.to_string(),
        }
    }

    async fn claim_with_worker(pool: &SqlitePool, worker_id: &str) -> spool_db::ClaimedRun {
        spool_db::claim_next(
            pool,
            &spool_db::ClaimNextInput {
                queue_key: "TASK".to_string(),
                worker_id: worker_id.to_string(),
                launcher_kind: "fake".to_string(),
                lease_seconds: 90,
            },
            &worker_actor(worker_id),
        )
        .await
        .expect("claim")
        .expect("claimed")
    }

    async fn finish_run(
        pool: &SqlitePool,
        run_id: &str,
        worker_id: &str,
        outcome: &str,
        failure_reason: Option<&str>,
    ) {
        spool_db::finish_run(
            pool,
            run_id,
            &spool_db::FinishRunInput {
                outcome: outcome.to_string(),
                failure_reason: failure_reason.map(str::to_string),
                failure_reason_code: None,
                retry_hold_seconds: (outcome == "failed").then_some(1),
            },
            &worker_actor(worker_id),
        )
        .await
        .expect("finish run");
    }

    fn options() -> MonitorOptions {
        MonitorOptions {
            queue: None,
            refresh_seconds: 5,
            plain: true,
            once: true,
            config_path: PathBuf::from("/config.toml"),
            data_dir: PathBuf::from("/data"),
            db_path: PathBuf::from("/spool.db"),
        }
    }

    #[tokio::test]
    async fn snapshot_includes_queue_counts_active_runs_and_recent_outcomes() {
        let pool = temp_pool().await;
        let identifier = seed_queue_and_task(&pool).await;
        let claimed = spool_db::claim_next(
            &pool,
            &spool_db::ClaimNextInput {
                queue_key: "TASK".to_string(),
                worker_id: "worker-1".to_string(),
                launcher_kind: "fake".to_string(),
                lease_seconds: 90,
            },
            &spool_db::Actor {
                kind: "worker_agent".to_string(),
                id: "worker-1".to_string(),
                display_name: "worker-1".to_string(),
            },
        )
        .await
        .expect("claim")
        .expect("claimed");

        let snapshot = load_snapshot(&pool, &options()).await.expect("snapshot");

        assert_eq!(snapshot.queues.len(), 1);
        let queue = &snapshot.queues[0];
        assert_eq!(queue.key, "TASK");
        assert_eq!(queue.active_agent_runs, 1);
        assert_eq!(queue.active_runs[0].task_identifier, identifier);
        assert_eq!(queue.active_runs[0].task_title, "Do work");
        assert_eq!(queue.ready_tasks.len(), 0);
        assert!(queue
            .state_counts
            .iter()
            .any(|(state, count)| state == "in_progress" && *count == 1));
        assert_eq!(snapshot.recent_runs[0].agent_run_id, claimed.run.id);
        assert_eq!(snapshot.recent_runs[0].outcome, None);
    }

    #[tokio::test]
    async fn snapshot_queue_filter_limits_output() {
        let pool = temp_pool().await;
        seed_queue_and_task(&pool).await;
        let mut filtered = options();
        filtered.queue = Some("OTHER".to_string());

        let snapshot = load_snapshot(&pool, &filtered).await.expect("snapshot");

        assert!(snapshot.queues.is_empty());
        assert!(snapshot.recent_runs.is_empty());
    }

    #[tokio::test]
    async fn recovered_failed_run_is_not_attention_but_stays_recent() {
        let pool = temp_pool().await;
        let identifier = seed_queue_and_task(&pool).await;
        let failed = claim_with_worker(&pool, "worker-failed").await;
        finish_run(
            &pool,
            &failed.run.id,
            "worker-failed",
            "failed",
            Some("first attempt failed"),
        )
        .await;
        sqlx::query("DELETE FROM task_retry_holds")
            .execute(&pool)
            .await
            .expect("clear retry hold");
        let recovered = claim_with_worker(&pool, "worker-recovered").await;
        finish_run(
            &pool,
            &recovered.run.id,
            "worker-recovered",
            "completed",
            None,
        )
        .await;

        let snapshot = load_snapshot(&pool, &options()).await.expect("snapshot");
        let attention = attention_texts(&snapshot).join("\n");
        let failed_run = snapshot
            .recent_runs
            .iter()
            .find(|run| run.agent_run_id == failed.run.id)
            .expect("failed run remains recent");

        assert_eq!(failed_run.task_identifier, identifier);
        assert!(failed_run.recovered_by_later_success);
        assert!(!attention.contains("failed run:"));
        let mut out = Vec::new();
        write_snapshot(&mut out, &snapshot).expect("write");
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.contains("Recent Agent Runs:"));
        assert!(text.contains("first attempt failed"));
    }

    #[tokio::test]
    async fn unrecovered_latest_failed_run_remains_attention() {
        let pool = temp_pool().await;
        seed_queue_and_task(&pool).await;
        let failed = claim_with_worker(&pool, "worker-failed").await;
        sqlx::query(
            "UPDATE agent_runs SET outcome = 'expired', failure_reason = 'lease expired', finished_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(&failed.run.id)
        .execute(&pool)
        .await
        .expect("mark expired");

        let snapshot = load_snapshot(&pool, &options()).await.expect("snapshot");
        let attention = attention_texts(&snapshot).join("\n");

        assert!(attention.contains("failed run:"));
        assert!(attention.contains("expired"));
        assert!(attention.contains("lease expired"));
    }

    #[tokio::test]
    async fn done_task_suppresses_older_failed_run_attention() {
        let pool = temp_pool().await;
        seed_queue_and_task(&pool).await;
        let failed = claim_with_worker(&pool, "worker-failed").await;
        finish_run(
            &pool,
            &failed.run.id,
            "worker-failed",
            "failed",
            Some("fixed outside run"),
        )
        .await;
        sqlx::query("DELETE FROM task_retry_holds")
            .execute(&pool)
            .await
            .expect("clear retry hold");
        sqlx::query("UPDATE tasks SET state = 'done'")
            .execute(&pool)
            .await
            .expect("mark done");

        let snapshot = load_snapshot(&pool, &options()).await.expect("snapshot");
        let failed_run = snapshot
            .recent_runs
            .iter()
            .find(|run| run.agent_run_id == failed.run.id)
            .expect("failed run remains recent");

        assert_eq!(failed_run.task_state, "done");
        assert!(attention_texts(&snapshot).is_empty());
    }

    #[tokio::test]
    async fn canceled_task_suppresses_failed_run_attention_but_keeps_recent_history() {
        let pool = temp_pool().await;
        seed_queue_and_task(&pool).await;
        let failed = claim_with_worker(&pool, "worker-failed").await;
        finish_run(
            &pool,
            &failed.run.id,
            "worker-failed",
            "failed",
            Some("abandoned after cleanup"),
        )
        .await;
        sqlx::query("DELETE FROM task_retry_holds")
            .execute(&pool)
            .await
            .expect("clear retry hold");
        sqlx::query("UPDATE tasks SET state = 'canceled'")
            .execute(&pool)
            .await
            .expect("mark canceled");

        let snapshot = load_snapshot(&pool, &options()).await.expect("snapshot");
        let failed_run = snapshot
            .recent_runs
            .iter()
            .find(|run| run.agent_run_id == failed.run.id)
            .expect("failed run remains recent");

        assert_eq!(failed_run.task_state, "canceled");
        assert!(attention_texts(&snapshot).is_empty());
        let mut out = Vec::new();
        write_snapshot(&mut out, &snapshot).expect("write");
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.contains("Recent Agent Runs:"));
        assert!(text.contains("abandoned after cleanup"));
    }

    #[test]
    fn plain_snapshot_output_includes_context_and_keybindings() {
        let snapshot = MonitorSnapshot {
            config_path: PathBuf::from("/repo/.spool/config.toml"),
            data_dir: PathBuf::from("/repo/.spool/data"),
            db_path: PathBuf::from("/repo/.spool/data/spool.db"),
            queue_filter: Some("TASK".to_string()),
            captured_at: "2026-05-09 00:00:00".to_string(),
            queues: Vec::new(),
            recent_runs: Vec::new(),
        };
        let mut out = Vec::new();

        write_snapshot(&mut out, &snapshot).expect("write");
        let text = String::from_utf8(out).expect("utf8");

        assert!(text.contains("Spool attention board"));
        assert!(text.contains("config: /repo/.spool/config.toml"));
        assert!(text.contains("data: /repo/.spool/data"));
        assert!(text.contains("database: /repo/.spool/data/spool.db"));
        assert!(text.contains("queue filter: TASK"));
        assert!(text.contains("keys: q quit, r refresh; use spool status for details"));
        assert!(text.contains("No Task Queues found"));
    }

    #[test]
    fn crlf_writer_expands_newline_for_raw_terminal_mode() {
        let mut out = Vec::new();

        write!(CrLfWriter::new(&mut out), "one\ntwo\n").expect("write");

        assert_eq!(String::from_utf8(out).expect("utf8"), "one\r\ntwo\r\n");
    }

    #[test]
    fn ratatui_tui_render_includes_context_and_task_queue_status() {
        let snapshot = MonitorSnapshot {
            config_path: PathBuf::from("/repo/.spool/config.toml"),
            data_dir: PathBuf::from("/repo/.spool/data"),
            db_path: PathBuf::from("/repo/.spool/data/spool.db"),
            queue_filter: Some("TASK".to_string()),
            captured_at: "2026-05-09 00:00:00".to_string(),
            queues: vec![QueueSnapshot {
                key: "TASK".to_string(),
                name: "Spool".to_string(),
                state_counts: vec![("ready".to_string(), 2), ("in_progress".to_string(), 1)],
                active_agent_runs: 1,
                active_retry_holds: 0,
                active_runs: Vec::new(),
                retry_holds: Vec::new(),
                ready_tasks: vec![spool_db::TaskStatusSummary {
                    queue_key: "TASK".to_string(),
                    identifier: "TASK-47".to_string(),
                    title: "Prepare monitor titles".to_string(),
                    state: "ready".to_string(),
                    priority: "normal".to_string(),
                    local_worktree: None,
                    task_branch: None,
                    main_branch: "main".to_string(),
                    latest_rework_reason_code: None,
                    latest_rework_reason: None,
                    unresolved_blocking_task_count: 0,
                    blocking_task_identifiers: None,
                }],
                rework_tasks: Vec::new(),
                human_review_tasks: Vec::new(),
                integrating_tasks: Vec::new(),
                advisory_conflict_hints: Vec::new(),
                integration_retries: Vec::new(),
                repo_operation_lock: None,
            }],
            recent_runs: vec![RecentRunSnapshot {
                queue_key: "TASK".to_string(),
                task_identifier: "TASK-46".to_string(),
                task_title: "Show useful Task titles".to_string(),
                agent_run_id: "run-1".to_string(),
                launcher_kind: "pi".to_string(),
                worker_id: "worker".to_string(),
                outcome: Some("completed".to_string()),
                failure_reason: None,
                failure_reason_code: None,
                task_state: "done".to_string(),
                recovered_by_later_success: false,
                created_at: "2026-05-09 00:00:00".to_string(),
                finished_at: Some("2026-05-09 00:01:00".to_string()),
            }],
        };
        let backend = ratatui::backend::TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_snapshot(frame, &snapshot))
            .expect("draw");
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains("Spool"));
        assert!(rendered.contains("attention board"));
        assert!(rendered.contains("Needs Attention"));
        assert!(rendered.contains("Running"));
        assert!(rendered.contains("Rework"));
        assert!(rendered.contains("Next"));
        assert!(rendered.contains("Advisory Task Conflict Hints"));
        assert!(rendered.contains("Prepare monitor titles"));
        assert!(rendered.contains("Recent Agent Runs"));
        assert!(rendered.contains("q quit"));
    }

    #[test]
    fn local_worktree_status_reports_dirty_branch_and_stale_main() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo dir");
        run_git(&repo, &["init", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "tester@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        fs::write(repo.join("file.txt"), "base\n").expect("write base");
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-m", "base"]);
        run_git(&repo, &["checkout", "-b", "spool/TASK-1"]);
        run_git(&repo, &["checkout", "main"]);
        fs::write(repo.join("main.txt"), "main\n").expect("write main");
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-m", "main advances"]);
        run_git(&repo, &["checkout", "spool/TASK-1"]);
        fs::write(repo.join("dirty.txt"), "dirty\n").expect("write dirty");

        let status = local_worktree_status(
            Some(repo.to_str().expect("utf8 path")),
            Some("spool/TASK-1"),
            "main",
        );

        assert!(status.contains("dirty"), "{status}");
        assert!(status.contains("branch=spool/TASK-1"), "{status}");
        assert!(status.contains("includes_main=no"), "{status}");
    }

    #[test]
    fn raw_terminal_snapshot_output_normalizes_all_newlines_to_crlf() {
        let snapshot = MonitorSnapshot {
            config_path: PathBuf::from("/repo/.spool/config.toml"),
            data_dir: PathBuf::from("/repo/.spool/data"),
            db_path: PathBuf::from("/repo/.spool/data/spool.db"),
            queue_filter: Some("TASK".to_string()),
            captured_at: "2026-05-09 00:00:00".to_string(),
            queues: vec![QueueSnapshot {
                key: "TASK".to_string(),
                name: "Spool".to_string(),
                state_counts: vec![("ready".to_string(), 1)],
                active_agent_runs: 0,
                active_retry_holds: 0,
                active_runs: Vec::new(),
                retry_holds: Vec::new(),
                ready_tasks: Vec::new(),
                rework_tasks: Vec::new(),
                human_review_tasks: Vec::new(),
                integrating_tasks: Vec::new(),
                advisory_conflict_hints: Vec::new(),
                integration_retries: Vec::new(),
                repo_operation_lock: None,
            }],
            recent_runs: Vec::new(),
        };
        let mut out = Vec::new();

        write_snapshot(CrLfWriter::new(&mut out), &snapshot).expect("write");
        let text = String::from_utf8(out).expect("utf8");

        assert!(text.contains("Spool attention board\r\n"));
        assert!(text.contains("\r\nNeeds Attention:\r\n"));
        assert!(text.contains("\r\nRunning:\r\n"));
        assert!(!text.contains("\n  ready: 1\n"));
    }

    #[test]
    fn attention_first_plain_output_orders_sections_and_limits_next_tasks() {
        let ready_tasks = (1..=7)
            .map(|index| spool_db::TaskStatusSummary {
                queue_key: "TASK".to_string(),
                identifier: format!("TASK-{index}"),
                title: format!("Ready task with a deliberately long title number {index}"),
                state: "ready".to_string(),
                priority: "normal".to_string(),
                local_worktree: None,
                task_branch: None,
                main_branch: "main".to_string(),
                latest_rework_reason_code: None,
                latest_rework_reason: None,
                unresolved_blocking_task_count: 0,
                blocking_task_identifiers: None,
            })
            .collect();
        let snapshot = MonitorSnapshot {
            config_path: PathBuf::from("/repo/.spool/config.toml"),
            data_dir: PathBuf::from("/repo/.spool/data"),
            db_path: PathBuf::from("/repo/.spool/data/spool.db"),
            queue_filter: Some("TASK".to_string()),
            captured_at: "2026-05-09 00:00:00".to_string(),
            queues: vec![QueueSnapshot {
                key: "TASK".to_string(),
                name: "Spool".to_string(),
                state_counts: vec![("ready".to_string(), 7)],
                active_agent_runs: 0,
                active_retry_holds: 1,
                active_runs: Vec::new(),
                retry_holds: vec![spool_db::ActiveRetryHoldStatus {
                    queue_key: "TASK".to_string(),
                    task_identifier: "TASK-99".to_string(),
                    hold_until: "2026-05-09 00:05:00".to_string(),
                    reason: "agent failed".to_string(),
                    failure_reason_code: Some("agent_run_failed".to_string()),
                }],
                ready_tasks,
                rework_tasks: Vec::new(),
                human_review_tasks: Vec::new(),
                integrating_tasks: Vec::new(),
                advisory_conflict_hints: vec![spool_db::TaskConflictGroup {
                    queue_key: "TASK".to_string(),
                    target: "crates/spool-cli".to_string(),
                    task_count: 2,
                    tasks: "TASK-1 (ready), TASK-2 (in_progress)".to_string(),
                }],
                integration_retries: Vec::new(),
                repo_operation_lock: None,
            }],
            recent_runs: Vec::new(),
        };
        let mut out = Vec::new();

        write_snapshot(&mut out, &snapshot).expect("write");
        let text = String::from_utf8(out).expect("utf8");

        assert!(text.find("Needs Attention:").unwrap() < text.find("Running:").unwrap());
        assert!(text.find("Running:").unwrap() < text.find("Rework:").unwrap());
        assert!(text.find("Rework:").unwrap() < text.find("Human Review:").unwrap());
        assert!(text.find("Human Review:").unwrap() < text.find("Next:").unwrap());
        assert!(text.find("Next:").unwrap() < text.find("Advisory Task Conflict Hints:").unwrap());
        assert!(
            text.find("Advisory Task Conflict Hints:").unwrap()
                < text.find("Recent Agent Runs:").unwrap()
        );
        assert!(text.contains("retry hold: TASK-99"));
        assert!(text.contains("… 2 more Ready Tasks in TASK"));
        assert!(text.contains("Advisory Task Conflict Hints:"));
        assert!(text.contains("crates/spool-cli\t2 Task(s): TASK-1 (ready), TASK-2 (in_progress)"));
        assert!(!text.contains("Ready task with a deliberately long title number 6\t"));
    }

    #[tokio::test]
    async fn snapshot_and_plain_output_include_human_review_blocking_ready_tasks() {
        let pool = temp_pool().await;
        let review_identifier = seed_queue_and_task(&pool).await;
        sqlx::query(
            "UPDATE tasks SET state = 'human_review', title = 'Review API boundary' WHERE identifier = ?",
        )
        .bind(&review_identifier)
        .execute(&pool)
        .await
        .expect("mark human review");
        let blocked = spool_db::CreateTask {
            queue_key: "TASK".to_string(),
            title: "Continue runner extraction".to_string(),
            brief: "brief".to_string(),
            priority: "normal".to_string(),
            state: "ready".to_string(),
            review_required: false,
            acceptance_criteria: vec!["criterion".to_string()],
            validation_items: vec!["validation".to_string()],
            tags: Vec::new(),
            conflict_hints: Vec::new(),
            blocking_task_identifiers: vec![review_identifier.clone()],
        };
        let blocked = spool_db::create_task(&pool, &blocked, &spool_db::Actor::operator("tester"))
            .await
            .expect("create blocked ready task");

        let snapshot = load_snapshot(&pool, &options()).await.expect("snapshot");
        let queue = &snapshot.queues[0];
        assert_eq!(queue.human_review_tasks.len(), 1);
        assert_eq!(queue.ready_tasks.len(), 1);

        let attention = attention_texts(&snapshot).join("\n");
        assert!(attention.contains("human review:"));
        assert!(attention.contains("Review API boundary"));
        assert!(attention.contains("blocks Ready Tasks"));
        assert!(attention.contains(&blocked.task.identifier));

        let mut out = Vec::new();
        write_snapshot(&mut out, &snapshot).expect("write");
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.contains("Human Review:"));
        assert!(text.contains("waiting for Review Decision"));
        assert!(text.contains(&format!("blocked_by={review_identifier}:human_review")));
    }

    #[test]
    fn attention_texts_cover_stale_failed_retry_and_integrating_items() {
        let snapshot = MonitorSnapshot {
            config_path: PathBuf::from("/config.toml"),
            data_dir: PathBuf::from("/data"),
            db_path: PathBuf::from("/spool.db"),
            queue_filter: None,
            captured_at: "2026-05-09 00:00:00".to_string(),
            queues: vec![QueueSnapshot {
                key: "TASK".to_string(),
                name: "Spool".to_string(),
                state_counts: Vec::new(),
                active_agent_runs: 1,
                active_retry_holds: 1,
                active_runs: vec![spool_db::ActiveAgentRunStatus {
                    queue_key: "TASK".to_string(),
                    task_identifier: "TASK-1".to_string(),
                    task_title: "Stale work".to_string(),
                    task_state: "in_progress".to_string(),
                    agent_run_id: "run-stale".to_string(),
                    launcher_kind: "pi".to_string(),
                    worker_id: "worker".to_string(),
                    lease_expires_at: "2026-05-08 23:59:59".to_string(),
                }],
                retry_holds: vec![spool_db::ActiveRetryHoldStatus {
                    queue_key: "TASK".to_string(),
                    task_identifier: "TASK-2".to_string(),
                    hold_until: "2026-05-09 00:10:00".to_string(),
                    reason: "retry later".to_string(),
                    failure_reason_code: Some("launcher_timeout".to_string()),
                }],
                ready_tasks: Vec::new(),
                rework_tasks: Vec::new(),
                human_review_tasks: Vec::new(),
                integrating_tasks: vec![spool_db::TaskStatusSummary {
                    queue_key: "TASK".to_string(),
                    identifier: "TASK-3".to_string(),
                    title: "Waiting integration".to_string(),
                    state: "integrating".to_string(),
                    priority: "normal".to_string(),
                    local_worktree: None,
                    task_branch: None,
                    main_branch: "main".to_string(),
                    latest_rework_reason_code: None,
                    latest_rework_reason: None,
                    unresolved_blocking_task_count: 0,
                    blocking_task_identifiers: None,
                }],
                advisory_conflict_hints: Vec::new(),
                integration_retries: vec![spool_db::IntegrationRetryStatus {
                    queue_key: "TASK".to_string(),
                    task_identifier: "TASK-4".to_string(),
                    task_title: "Retry integration".to_string(),
                    reason_code: "unknown_operational_failure".to_string(),
                    retryable: false,
                    retry_attempt: Some(2),
                    next_retry_at: None,
                    reason: Some("operator needed".to_string()),
                }],
                repo_operation_lock: Some("TASK pid=123 operation=manual".to_string()),
            }],
            recent_runs: vec![RecentRunSnapshot {
                queue_key: "TASK".to_string(),
                task_identifier: "TASK-5".to_string(),
                task_title: "Failed agent run".to_string(),
                agent_run_id: "run-failed".to_string(),
                launcher_kind: "pi".to_string(),
                worker_id: "worker".to_string(),
                outcome: Some("failed".to_string()),
                failure_reason: Some("boom".to_string()),
                failure_reason_code: Some("agent_run_failed".to_string()),
                task_state: "in_progress".to_string(),
                recovered_by_later_success: false,
                created_at: "2026-05-09 00:00:00".to_string(),
                finished_at: Some("2026-05-09 00:01:00".to_string()),
            }],
        };

        let attention = attention_texts(&snapshot).join("\n");

        assert!(attention.contains("repo lock:"));
        assert!(attention.contains("stale run:"));
        assert!(attention.contains("retry hold:"));
        assert!(attention.contains("integrating:"));
        assert!(attention.contains("integration:"));
        assert!(attention.contains("failed run:"));
    }

    #[test]
    fn healthy_active_runs_are_compact_and_not_attention() {
        let snapshot = MonitorSnapshot {
            config_path: PathBuf::from("/config.toml"),
            data_dir: PathBuf::from("/data"),
            db_path: PathBuf::from("/spool.db"),
            queue_filter: None,
            captured_at: "2026-05-09 00:00:00".to_string(),
            queues: vec![QueueSnapshot {
                key: "TASK".to_string(),
                name: "Spool".to_string(),
                state_counts: Vec::new(),
                active_agent_runs: 1,
                active_retry_holds: 0,
                active_runs: vec![spool_db::ActiveAgentRunStatus {
                    queue_key: "TASK".to_string(),
                    task_identifier: "TASK-1".to_string(),
                    task_title: "Healthy active run with verbose title".to_string(),
                    task_state: "in_progress".to_string(),
                    agent_run_id: "1234567890abcdef".to_string(),
                    launcher_kind: "pi".to_string(),
                    worker_id: "worker".to_string(),
                    lease_expires_at: "2026-05-09 00:01:00".to_string(),
                }],
                retry_holds: Vec::new(),
                ready_tasks: Vec::new(),
                rework_tasks: Vec::new(),
                human_review_tasks: Vec::new(),
                integrating_tasks: Vec::new(),
                advisory_conflict_hints: Vec::new(),
                integration_retries: Vec::new(),
                repo_operation_lock: None,
            }],
            recent_runs: Vec::new(),
        };
        let mut out = Vec::new();

        write_snapshot(&mut out, &snapshot).expect("write");
        let text = String::from_utf8(out).expect("utf8");

        assert!(attention_texts(&snapshot).is_empty());
        assert!(text.contains("● TASK-1"));
        assert!(text.contains("run=12345678"));
    }

    fn task_summary(identifier: &str, title: &str, state: &str) -> spool_db::TaskStatusSummary {
        spool_db::TaskStatusSummary {
            queue_key: "TASK".to_string(),
            identifier: identifier.to_string(),
            title: title.to_string(),
            state: state.to_string(),
            priority: "normal".to_string(),
            local_worktree: None,
            task_branch: Some(format!("spool/{identifier}")),
            main_branch: "main".to_string(),
            latest_rework_reason_code: Some("merge_conflict".to_string()),
            latest_rework_reason: Some(
                "very long merge conflict diagnosis that belongs in Rework details".to_string(),
            ),
            unresolved_blocking_task_count: 0,
            blocking_task_identifiers: None,
        }
    }

    fn active_run(identifier: &str, title: &str, state: &str) -> spool_db::ActiveAgentRunStatus {
        spool_db::ActiveAgentRunStatus {
            queue_key: "TASK".to_string(),
            task_identifier: identifier.to_string(),
            task_title: title.to_string(),
            task_state: state.to_string(),
            agent_run_id: format!("run-{identifier}"),
            launcher_kind: "pi".to_string(),
            worker_id: "worker".to_string(),
            lease_expires_at: "2026-05-09 00:01:00".to_string(),
        }
    }

    fn snapshot_with_queue(queue: QueueSnapshot) -> MonitorSnapshot {
        MonitorSnapshot {
            config_path: PathBuf::from("/config.toml"),
            data_dir: PathBuf::from("/data"),
            db_path: PathBuf::from("/spool.db"),
            queue_filter: Some("TASK".to_string()),
            captured_at: "2026-05-09 00:00:00".to_string(),
            queues: vec![queue],
            recent_runs: Vec::new(),
        }
    }

    fn empty_queue() -> QueueSnapshot {
        QueueSnapshot {
            key: "TASK".to_string(),
            name: "Spool".to_string(),
            state_counts: Vec::new(),
            active_agent_runs: 0,
            active_retry_holds: 0,
            active_runs: Vec::new(),
            retry_holds: Vec::new(),
            ready_tasks: Vec::new(),
            rework_tasks: Vec::new(),
            human_review_tasks: Vec::new(),
            integrating_tasks: Vec::new(),
            advisory_conflict_hints: Vec::new(),
            integration_retries: Vec::new(),
            repo_operation_lock: None,
        }
    }

    #[test]
    fn active_rework_run_is_running_not_noisy_attention() {
        let mut queue = empty_queue();
        queue.active_agent_runs = 1;
        queue.active_runs = vec![active_run("TASK-7", "Fix merge conflict", "rework")];
        queue.rework_tasks = vec![task_summary("TASK-7", "Fix merge conflict", "rework")];
        let snapshot = snapshot_with_queue(queue);
        let mut out = Vec::new();

        write_snapshot(&mut out, &snapshot).expect("write");
        let text = String::from_utf8(out).expect("utf8");

        assert!(attention_texts(&snapshot).is_empty());
        assert!(text.contains("state=rework in progress"));
        assert!(text.contains("Rework:"));
        assert!(text.contains("rework in progress"));
        assert!(text.contains("reason: very long merge conflict diagnosis"));
        assert!(!text.contains("rework: TASK-7"));
    }

    #[test]
    fn unattended_rework_is_concise_attention_with_details_in_rework() {
        let mut queue = empty_queue();
        queue.rework_tasks = vec![task_summary("TASK-8", "Needs operator recovery", "rework")];
        let snapshot = snapshot_with_queue(queue);
        let attention = attention_texts(&snapshot).join("\n");
        let mut out = Vec::new();

        write_snapshot(&mut out, &snapshot).expect("write");
        let text = String::from_utf8(out).expect("utf8");

        assert!(attention.contains("rework: TASK-8"));
        assert!(attention.contains("waiting for Worker Agent/operator; code=merge_conflict"));
        assert!(!attention.contains("very long merge conflict diagnosis"));
        assert!(text.contains("local_worktree=missing Task Link"));
        assert!(text.contains("reason: very long merge conflict diagnosis"));
    }

    #[test]
    fn ratatui_advisory_hints_are_separate_from_next() {
        let mut queue = empty_queue();
        queue.ready_tasks = vec![task_summary("TASK-9", "Ready implementation", "ready")];
        queue.advisory_conflict_hints = vec![spool_db::TaskConflictGroup {
            queue_key: "TASK".to_string(),
            target: "crates/spool-cli".to_string(),
            task_count: 2,
            tasks: "TASK-9 (ready), TASK-10 (in_progress)".to_string(),
        }];
        let snapshot = snapshot_with_queue(queue);
        let backend = ratatui::backend::TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_snapshot(frame, &snapshot))
            .expect("draw");
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains("Next"));
        assert!(rendered.contains("Ready implementation"));
        assert!(rendered.contains("Advisory Task Conflict Hints"));
        assert!(rendered.contains("crates/spool-cli"));
        assert!(!rendered.contains("hotspot"));
    }

    #[test]
    fn ratatui_attention_shows_human_review_and_blocked_ready_task() {
        let mut queue = empty_queue();
        queue.human_review_tasks = vec![task_summary(
            "TASK-11",
            "Review public runner API",
            "human_review",
        )];
        let mut ready = task_summary("TASK-12", "Continue extraction", "ready");
        ready.unresolved_blocking_task_count = 1;
        ready.blocking_task_identifiers = Some("TASK-11:human_review".to_string());
        queue.ready_tasks = vec![ready];
        let snapshot = snapshot_with_queue(queue);
        let backend = ratatui::backend::TestBackend::new(140, 32);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_snapshot(frame, &snapshot))
            .expect("draw");
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains("human review"));
        assert!(rendered.contains("Review public runner API"));
        assert!(rendered.contains("blocks Ready Tasks"));
        assert!(rendered.contains("blocked_by=TASK-11:human_review"));
    }

    #[test]
    fn terminal_ui_capability_rejects_non_terminal_or_dumb_terminal() {
        assert!(terminal_supports_ui(true, None));
        assert!(terminal_supports_ui(true, Some("xterm-256color")));
        assert!(terminal_supports_ui(true, Some("tmux-256color")));
        assert!(terminal_supports_ui(true, Some("screen-256color")));
        assert!(!terminal_supports_ui(false, Some("xterm-256color")));
        assert!(!terminal_supports_ui(true, Some("dumb")));
    }
}

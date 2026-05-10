use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Write},
    path::PathBuf,
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
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
    Frame, Terminal,
};
use sqlx::SqlitePool;

use crate::display;

#[derive(Debug, Clone)]
pub struct MonitorOptions {
    pub queue: Option<String>,
    pub refresh_seconds: u64,
    pub plain: bool,
    pub once: bool,
    pub config_path: PathBuf,
    pub db_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorSnapshot {
    pub config_path: PathBuf,
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
    pub active_runs: Vec<tasker_db::ActiveAgentRunStatus>,
    pub retry_holds: Vec<tasker_db::ActiveRetryHoldStatus>,
    pub ready_tasks: Vec<tasker_db::TaskStatusSummary>,
    pub integrating_tasks: Vec<tasker_db::TaskStatusSummary>,
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
    pub created_at: String,
    pub finished_at: Option<String>,
}

pub async fn run_monitor(pool: &SqlitePool, options: MonitorOptions) -> Result<()> {
    if options.plain || options.once || !stdout_supports_terminal_ui() {
        let snapshot = load_snapshot(pool, &options).await?;
        write_snapshot(io::stdout(), &snapshot)?;
        if !options.once && !options.plain && !stdout_supports_terminal_ui() {
            eprintln!(
                "tasker monitor: stdout is not an interactive terminal; printed one plain snapshot"
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
            Constraint::Length(5),
            Constraint::Percentage(38),
            Constraint::Percentage(34),
            Constraint::Length(3),
        ])
        .split(frame.area());

    render_header(frame, chunks[0], snapshot);
    render_queues(frame, chunks[1], snapshot);
    render_recent_runs(frame, chunks[2], snapshot);
    render_footer(frame, chunks[3]);
}

fn render_header(frame: &mut Frame<'_>, area: Rect, snapshot: &MonitorSnapshot) {
    let queue = snapshot.queue_filter.as_deref().unwrap_or("all");
    let lines = vec![
        Line::from("Tasker terminal status monitor"),
        Line::from(format!(
            "captured at: {} | queue filter: {queue}",
            snapshot.captured_at
        )),
        Line::from(format!("config: {}", snapshot.config_path.display())),
        Line::from(format!("database: {}", snapshot.db_path.display())),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title("Context").borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_queues(frame: &mut Frame<'_>, area: Rect, snapshot: &MonitorSnapshot) {
    if snapshot.queues.is_empty() {
        frame.render_widget(
            Paragraph::new("No Task Queues found")
                .block(Block::default().title("Task Queues").borders(Borders::ALL)),
            area,
        );
        return;
    }

    let rows = snapshot.queues.iter().map(|queue| {
        let state_counts = queue
            .state_counts
            .iter()
            .map(|(state, count)| format!("{state}:{count}"))
            .collect::<Vec<_>>()
            .join("  ");
        Row::new(vec![
            Cell::from(queue.key.clone()),
            Cell::from(queue.name.clone()),
            Cell::from(state_counts),
            Cell::from(queue.active_agent_runs.to_string()),
            Cell::from(queue.active_retry_holds.to_string()),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Length(22),
            Constraint::Min(24),
            Constraint::Length(11),
            Constraint::Length(11),
        ],
    )
    .header(
        Row::new(["Queue", "Name", "Task States", "Agent Runs", "Retry Holds"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().title("Task Queues").borders(Borders::ALL));
    frame.render_widget(table, area);
}

fn render_recent_runs(frame: &mut Frame<'_>, area: Rect, snapshot: &MonitorSnapshot) {
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let mut run_lines = Vec::new();
    for queue in &snapshot.queues {
        for run in &queue.active_runs {
            run_lines.push(Line::from(format!(
                "{} {} launcher={} worker={} lease={}",
                display::task_label(&run.task_identifier, &run.task_title, 40),
                run.agent_run_id,
                run.launcher_kind,
                run.worker_id,
                run.lease_expires_at
            )));
        }
        for hold in &queue.retry_holds {
            run_lines.push(Line::from(format!(
                "{} retry hold until {} reason={}",
                hold.task_identifier, hold.hold_until, hold.reason
            )));
        }
        for task in &queue.ready_tasks {
            run_lines.push(Line::from(format!(
                "ready {} priority={}",
                display::task_label(&task.identifier, &task.title, 40),
                task.priority
            )));
        }
        for task in &queue.integrating_tasks {
            run_lines.push(Line::from(format!(
                "integrating {} priority={}",
                display::task_label(&task.identifier, &task.title, 40),
                task.priority
            )));
        }
    }
    if run_lines.is_empty() {
        run_lines.push(Line::from(
            "No active Agent Runs, Retry Holds, Ready Tasks, or Integrating Tasks",
        ));
    }
    frame.render_widget(
        Paragraph::new(run_lines)
            .block(Block::default().title("Active Work").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        panes[0],
    );

    let recent_rows = snapshot.recent_runs.iter().map(|run| {
        Row::new(vec![
            Cell::from(run.queue_key.clone()),
            Cell::from(display::task_label(
                &run.task_identifier,
                &run.task_title,
                28,
            )),
            Cell::from(run.outcome.clone().unwrap_or_else(|| "active".to_string())),
            Cell::from(run.launcher_kind.clone()),
            Cell::from(run.worker_id.clone()),
            Cell::from(run.finished_at.clone().unwrap_or_else(|| "-".to_string())),
        ])
    });
    let table = Table::new(
        recent_rows,
        [
            Constraint::Length(8),
            Constraint::Length(30),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Min(10),
        ],
    )
    .header(
        Row::new(["Queue", "Task", "Outcome", "Launcher", "Worker", "Finished"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(
        Block::default()
            .title("Recent Agent Runs")
            .borders(Borders::ALL),
    );
    frame.render_widget(table, panes[1]);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new("Read-only. Keys: q/Esc/Ctrl-C quit, r refresh. Use --plain or --once for script-friendly snapshots.")
            .block(Block::default().borders(Borders::ALL)),
        area,
    );
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
    let mut rows = tasker_db::status_by_queue_and_state(pool).await?;
    let mut active_runs = tasker_db::active_agent_runs_for_status(pool).await?;
    let mut retry_holds = tasker_db::active_retry_holds_for_status(pool).await?;
    let mut status_tasks =
        tasker_db::tasks_for_status_by_states(pool, &["ready", "integrating"]).await?;
    let recent_runs = recent_agent_runs(pool, options.queue.as_deref()).await?;

    if let Some(queue) = &options.queue {
        rows.retain(|row| row.queue_key == *queue);
        active_runs.retain(|run| run.queue_key == *queue);
        retry_holds.retain(|hold| hold.queue_key == *queue);
        status_tasks.retain(|task| task.queue_key == *queue);
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
                integrating_tasks: Vec::new(),
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
                "integrating" => queue.integrating_tasks.push(task),
                _ => {}
            }
        }
    }

    Ok(MonitorSnapshot {
        config_path: options.config_path.clone(),
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
    query.push(" ORDER BY agent_runs.created_at DESC, agent_runs.id DESC LIMIT 10");
    query
        .build_query_as::<RecentRunSnapshot>()
        .fetch_all(pool)
        .await
        .context("failed to load recent Agent Runs")
}

pub fn write_snapshot(mut writer: impl Write, snapshot: &MonitorSnapshot) -> io::Result<()> {
    writeln!(writer, "Tasker terminal status monitor")?;
    writeln!(writer, "captured at: {}", snapshot.captured_at)?;
    writeln!(writer, "config: {}", snapshot.config_path.display())?;
    writeln!(writer, "database: {}", snapshot.db_path.display())?;
    if let Some(queue) = &snapshot.queue_filter {
        writeln!(writer, "queue filter: {queue}")?;
    }
    writeln!(writer, "keys: q/esc quit, r refresh, Ctrl-C quit")?;

    if snapshot.queues.is_empty() {
        writeln!(writer, "\nNo Task Queues found")?;
    }
    for queue in &snapshot.queues {
        writeln!(writer, "\nTask Queue: {}\t{}", queue.key, queue.name)?;
        writeln!(writer, "  active Agent Runs: {}", queue.active_agent_runs)?;
        for run in &queue.active_runs {
            writeln!(
                writer,
                "    {}\t{}\tlauncher={}\tworker={}\tlease_expires_at={}",
                display::task_label(&run.task_identifier, &run.task_title, 64),
                run.agent_run_id,
                run.launcher_kind,
                run.worker_id,
                run.lease_expires_at
            )?;
        }
        writeln!(writer, "  active Retry Holds: {}", queue.active_retry_holds)?;
        for hold in &queue.retry_holds {
            writeln!(
                writer,
                "    {}\thold_until={}\treason={}",
                hold.task_identifier, hold.hold_until, hold.reason
            )?;
        }
        for task in &queue.ready_tasks {
            writeln!(
                writer,
                "    ready {}\tpriority={}",
                display::task_label(&task.identifier, &task.title, 64),
                task.priority
            )?;
        }
        for task in &queue.integrating_tasks {
            writeln!(
                writer,
                "    integrating {}\tpriority={}",
                display::task_label(&task.identifier, &task.title, 64),
                task.priority
            )?;
        }
        for (state, count) in &queue.state_counts {
            writeln!(writer, "  {state}: {count}")?;
        }
    }

    writeln!(writer, "\nRecent Agent Runs:")?;
    if snapshot.recent_runs.is_empty() {
        writeln!(writer, "  (none)")?;
    }
    for run in &snapshot.recent_runs {
        let status = run.outcome.as_deref().unwrap_or("active");
        writeln!(
            writer,
            "  {}\t{}\t{}\tlauncher={}\tworker={}\tcreated={}\tfinished={}",
            run.queue_key,
            display::task_label(&run.task_identifier, &run.task_title, 64),
            status,
            run.launcher_kind,
            run.worker_id,
            run.created_at,
            run.finished_at.as_deref().unwrap_or("-")
        )?;
        if let Some(reason) = &run.failure_reason {
            writeln!(writer, "    failure reason: {reason}")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn temp_pool() -> SqlitePool {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        // Keep the tempdir alive by leaking it for the duration of the test process.
        let _temp = Box::leak(Box::new(temp));
        let pool = tasker_db::connect(&db_path).await.expect("connect db");
        tasker_db::run_migrations(&pool).await.expect("migrations");
        pool
    }

    async fn seed_queue_and_task(pool: &SqlitePool) -> String {
        tasker_db::create_task_queue(
            pool,
            &tasker_db::CreateTaskQueue {
                key: "TASK".to_string(),
                name: "Tasker".to_string(),
                managed_source_repository: "/repo".to_string(),
                main_branch: "main".to_string(),
                worktree_root: "/worktrees".to_string(),
                branch_template: "tasker/{task_identifier}".to_string(),
                done_worktree_retention: false,
                queue_concurrency_limit: Some(1),
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("create queue");
        let task = tasker_db::create_task(
            pool,
            &tasker_db::CreateTask {
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
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("create task");
        task.task.identifier
    }

    fn options() -> MonitorOptions {
        MonitorOptions {
            queue: None,
            refresh_seconds: 5,
            plain: true,
            once: true,
            config_path: PathBuf::from("/config.toml"),
            db_path: PathBuf::from("/tasker.db"),
        }
    }

    #[tokio::test]
    async fn snapshot_includes_queue_counts_active_runs_and_recent_outcomes() {
        let pool = temp_pool().await;
        let identifier = seed_queue_and_task(&pool).await;
        let claimed = tasker_db::claim_next(
            &pool,
            &tasker_db::ClaimNextInput {
                queue_key: "TASK".to_string(),
                worker_id: "worker-1".to_string(),
                launcher_kind: "fake".to_string(),
                lease_seconds: 90,
            },
            &tasker_db::Actor {
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

    #[test]
    fn plain_snapshot_output_includes_context_and_keybindings() {
        let snapshot = MonitorSnapshot {
            config_path: PathBuf::from("/repo/.tasker/config.toml"),
            db_path: PathBuf::from("/repo/.tasker/data/tasker.db"),
            queue_filter: Some("TASK".to_string()),
            captured_at: "2026-05-09 00:00:00".to_string(),
            queues: Vec::new(),
            recent_runs: Vec::new(),
        };
        let mut out = Vec::new();

        write_snapshot(&mut out, &snapshot).expect("write");
        let text = String::from_utf8(out).expect("utf8");

        assert!(text.contains("Tasker terminal status monitor"));
        assert!(text.contains("config: /repo/.tasker/config.toml"));
        assert!(text.contains("database: /repo/.tasker/data/tasker.db"));
        assert!(text.contains("queue filter: TASK"));
        assert!(text.contains("keys: q/esc quit, r refresh, Ctrl-C quit"));
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
            config_path: PathBuf::from("/repo/.tasker/config.toml"),
            db_path: PathBuf::from("/repo/.tasker/data/tasker.db"),
            queue_filter: Some("TASK".to_string()),
            captured_at: "2026-05-09 00:00:00".to_string(),
            queues: vec![QueueSnapshot {
                key: "TASK".to_string(),
                name: "Tasker".to_string(),
                state_counts: vec![("ready".to_string(), 2), ("in_progress".to_string(), 1)],
                active_agent_runs: 1,
                active_retry_holds: 0,
                active_runs: Vec::new(),
                retry_holds: Vec::new(),
                ready_tasks: vec![tasker_db::TaskStatusSummary {
                    queue_key: "TASK".to_string(),
                    identifier: "TASK-47".to_string(),
                    title: "Prepare monitor titles".to_string(),
                    state: "ready".to_string(),
                    priority: "normal".to_string(),
                }],
                integrating_tasks: Vec::new(),
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
                created_at: "2026-05-09 00:00:00".to_string(),
                finished_at: Some("2026-05-09 00:01:00".to_string()),
            }],
        };
        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_snapshot(frame, &snapshot))
            .expect("draw");
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains("Tasker terminal status monitor"));
        assert!(rendered.contains("config: /repo/.tasker/config.toml"));
        assert!(rendered.contains("TASK"));
        assert!(rendered.contains("ready:2"));
        assert!(rendered.contains("Prepare monitor titles"));
        assert!(rendered.contains("Recent Agent Runs"));
        assert!(rendered.contains("Read-only"));
    }

    #[test]
    fn raw_terminal_snapshot_output_normalizes_all_newlines_to_crlf() {
        let snapshot = MonitorSnapshot {
            config_path: PathBuf::from("/repo/.tasker/config.toml"),
            db_path: PathBuf::from("/repo/.tasker/data/tasker.db"),
            queue_filter: Some("TASK".to_string()),
            captured_at: "2026-05-09 00:00:00".to_string(),
            queues: vec![QueueSnapshot {
                key: "TASK".to_string(),
                name: "Tasker".to_string(),
                state_counts: vec![("ready".to_string(), 1)],
                active_agent_runs: 0,
                active_retry_holds: 0,
                active_runs: Vec::new(),
                retry_holds: Vec::new(),
                ready_tasks: Vec::new(),
                integrating_tasks: Vec::new(),
            }],
            recent_runs: Vec::new(),
        };
        let mut out = Vec::new();

        write_snapshot(CrLfWriter::new(&mut out), &snapshot).expect("write");
        let text = String::from_utf8(out).expect("utf8");

        assert!(text.contains("Tasker terminal status monitor\r\n"));
        assert!(text.contains("\r\nTask Queue: TASK\tTasker\r\n"));
        assert!(!text.contains("\n  ready: 1\n"));
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

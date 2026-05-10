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
    execute,
    terminal::{self, ClearType},
};
use sqlx::SqlitePool;

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
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RecentRunSnapshot {
    pub queue_key: String,
    pub task_identifier: String,
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
    let result = run_terminal_loop(pool, &options, &mut stdout).await;
    let cleanup = execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen)
        .context("failed to leave terminal monitor")
        .and_then(|_| terminal::disable_raw_mode().context("failed to disable terminal raw mode"));
    result.and(cleanup)
}

async fn run_terminal_loop(
    pool: &SqlitePool,
    options: &MonitorOptions,
    stdout: &mut io::Stdout,
) -> Result<()> {
    let refresh = Duration::from_secs(options.refresh_seconds.max(1));
    loop {
        let snapshot = load_snapshot(pool, options).await?;
        execute!(
            stdout,
            cursor::MoveTo(0, 0),
            terminal::Clear(ClearType::All)
        )
        .context("failed to redraw terminal monitor")?;
        write_snapshot(CrLfWriter::new(&mut *stdout), &snapshot)?;
        stdout.flush()?;

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

struct CrLfWriter<W> {
    inner: W,
}

impl<W> CrLfWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner }
    }
}

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
    let recent_runs = recent_agent_runs(pool, options.queue.as_deref()).await?;

    if let Some(queue) = &options.queue {
        rows.retain(|row| row.queue_key == *queue);
        active_runs.retain(|run| run.queue_key == *queue);
        retry_holds.retain(|hold| hold.queue_key == *queue);
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
                run.task_identifier,
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
            run.task_identifier,
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

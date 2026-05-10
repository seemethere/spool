use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::Deserialize;
use sqlx::{FromRow, SqlitePool};
use tokio::time::sleep;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorOptions {
    pub queue: String,
    pub concurrency: usize,
    pub timeout_seconds: u64,
    pub poll_seconds: u64,
    pub worker_command: Vec<String>,
    #[cfg(test)]
    pub run_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorOutcome {
    pub started_workers: usize,
    pub completed_workers: usize,
    pub failed_workers: usize,
    pub no_eligible_exits: usize,
    pub completed_handoffs: usize,
    pub blocked_reports: usize,
    pub retryable_failure_reports: usize,
    pub stuck_runs: Vec<StuckRun>,
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StuckRun {
    pub task_identifier: String,
    pub agent_run_id: String,
    pub worker_id: String,
}

struct WorkerProcess {
    actor: String,
    child: Child,
    stdout: Option<std::process::ChildStdout>,
    stderr: Option<std::process::ChildStderr>,
}

struct FinishedWorker {
    actor: String,
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

pub async fn supervise_batch(
    pool: &SqlitePool,
    options: SupervisorOptions,
) -> Result<SupervisorOutcome> {
    anyhow::ensure!(
        options.concurrency > 0,
        "--concurrency must be greater than zero"
    );
    anyhow::ensure!(
        options.timeout_seconds > 0,
        "--timeout-seconds must be greater than zero"
    );
    anyhow::ensure!(
        options.poll_seconds > 0,
        "--poll-seconds must be greater than zero"
    );
    anyhow::ensure!(
        !options.worker_command.is_empty(),
        "worker command must not be empty"
    );

    let deadline = Instant::now() + Duration::from_secs(options.timeout_seconds);
    let run_prefix = supervisor_run_prefix(&options);
    let status_dir = supervisor_status_dir(&run_prefix)?;
    println!("supervisor run prefix: {run_prefix}");
    let mut reports = SupervisorReports::default();
    let mut next_worker = 0usize;
    let mut saw_no_eligible = false;
    let mut active: Vec<WorkerProcess> = Vec::new();
    let mut outcome = SupervisorOutcome {
        started_workers: 0,
        completed_workers: 0,
        failed_workers: 0,
        no_eligible_exits: 0,
        completed_handoffs: 0,
        blocked_reports: 0,
        retryable_failure_reports: 0,
        stuck_runs: Vec::new(),
        timed_out: false,
    };

    while Instant::now() < deadline {
        reports.refresh(&mut outcome)?;
        let unblock = unblocking_state(pool, &options.queue, &reports).await?;
        if active.is_empty() && outcome.started_workers > 0 && unblock.should_stop() {
            println!("{} for Task Queue {}", unblock.reason(), options.queue);
            return Ok(outcome);
        }

        while !saw_no_eligible
            && !unblock.has_only_reported_work
            && active.len() < options.concurrency
        {
            next_worker += 1;
            let actor = supervisor_worker_id(&run_prefix, next_worker);
            let status_path = status_dir.join(format!("{actor}.jsonl"));
            reports.files.insert(status_path.clone());
            let worker = spawn_worker(&options.worker_command, &actor, status_path)
                .with_context(|| format!("failed to start worker {actor}"))?;
            println!("{}", started_worker_message(&actor));
            active.push(worker);
            outcome.started_workers += 1;
        }

        print_progress(pool, &options.queue).await?;

        let mut index = 0;
        while index < active.len() {
            if active[index].child.try_wait()?.is_some() {
                let finished = finish_worker(active.remove(index))?;
                reports.refresh(&mut outcome)?;
                outcome.completed_workers += 1;
                if !finished.status.success() {
                    outcome.failed_workers += 1;
                }
                if finished.stdout.contains("no eligible Tasks found") {
                    outcome.no_eligible_exits += 1;
                    saw_no_eligible = true;
                    println!("worker {} exited with no eligible Task", finished.actor);
                } else {
                    println!(
                        "worker {} exited status={}{}",
                        finished.actor,
                        finished.status,
                        concise_tail(&finished.stdout)
                    );
                }
                if !finished.stderr.trim().is_empty() {
                    eprintln!(
                        "worker {} stderr:{}",
                        finished.actor,
                        concise_tail(&finished.stderr)
                    );
                }

                let stuck = active_runs_for_worker(pool, &options.queue, &finished.actor).await?;
                for run in stuck {
                    println!(
                        "stuck Agent Run {} for Task {} remains active after worker {} exited; suggested recovery: tasker run fail {} --reason <reason>",
                        run.agent_run_id, run.task_identifier, run.worker_id, run.agent_run_id
                    );
                    outcome.stuck_runs.push(StuckRun {
                        task_identifier: run.task_identifier,
                        agent_run_id: run.agent_run_id,
                        worker_id: run.worker_id,
                    });
                }
            } else {
                index += 1;
            }
        }

        reports.refresh(&mut outcome)?;
        let unblock = unblocking_state(pool, &options.queue, &reports).await?;
        if active.is_empty() && (saw_no_eligible || unblock.should_stop()) {
            let reason = if saw_no_eligible {
                "drained queue"
            } else {
                unblock.reason()
            };
            println!("{reason} for Task Queue {}", options.queue);
            return Ok(outcome);
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        sleep(Duration::from_secs(options.poll_seconds).min(remaining)).await;
    }

    outcome.timed_out = true;
    println!(
        "supervisor timeout reached for Task Queue {}",
        options.queue
    );
    for mut worker in active {
        let _ = worker.child.kill();
        let _ = worker.child.wait();
    }
    Ok(outcome)
}

fn spawn_worker(command: &[String], actor: &str, status_path: PathBuf) -> Result<WorkerProcess> {
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..])
        .arg("--actor")
        .arg(actor)
        .env("TASKER_WORKER_STATUS_PATH", &status_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    Ok(WorkerProcess {
        actor: actor.to_string(),
        child,
        stdout,
        stderr,
    })
}

fn finish_worker(mut worker: WorkerProcess) -> Result<FinishedWorker> {
    let status = worker.child.wait()?;
    let mut stdout = String::new();
    if let Some(mut pipe) = worker.stdout.take() {
        pipe.read_to_string(&mut stdout)?;
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = worker.stderr.take() {
        pipe.read_to_string(&mut stderr)?;
    }
    Ok(FinishedWorker {
        actor: worker.actor,
        status,
        stdout,
        stderr,
    })
}

#[derive(Default)]
struct SupervisorReports {
    files: HashSet<PathBuf>,
    seen: HashSet<String>,
    reported_tasks: HashSet<String>,
    by_status: HashMap<String, usize>,
}

#[derive(Debug, Deserialize)]
struct WorkerStatusReport {
    tasker_worker_status: Option<bool>,
    task_identifier: Option<String>,
    agent_run_id: Option<String>,
    status: Option<String>,
    message: Option<String>,
}

impl SupervisorReports {
    fn refresh(&mut self, outcome: &mut SupervisorOutcome) -> Result<()> {
        for file in &self.files {
            let Ok(text) = fs::read_to_string(file) else {
                continue;
            };
            for line in text.lines() {
                let Ok(report) = serde_json::from_str::<WorkerStatusReport>(line) else {
                    continue;
                };
                if report.tasker_worker_status != Some(true) {
                    continue;
                }
                let task_identifier = report.task_identifier.unwrap_or_default();
                let status = report.status.unwrap_or_default();
                let key = format!(
                    "{}:{}:{}",
                    task_identifier,
                    report.agent_run_id.unwrap_or_default(),
                    status
                );
                if !self.seen.insert(key) {
                    continue;
                }
                if matches!(
                    status.as_str(),
                    "completion_intent" | "blocked" | "retryable_failure"
                ) {
                    self.reported_tasks.insert(task_identifier.clone());
                }
                *self.by_status.entry(status.clone()).or_insert(0) += 1;
                outcome.completed_handoffs = *self.by_status.get("completion_intent").unwrap_or(&0);
                outcome.blocked_reports = *self.by_status.get("blocked").unwrap_or(&0);
                outcome.retryable_failure_reports =
                    *self.by_status.get("retryable_failure").unwrap_or(&0);
                println!(
                    "worker status report Task {} status={}{}",
                    task_identifier,
                    status,
                    report
                        .message
                        .map(|message| format!(" message={message}"))
                        .unwrap_or_default()
                );
            }
        }
        Ok(())
    }
}

static SUPERVISOR_RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

fn supervisor_run_prefix(_options: &SupervisorOptions) -> String {
    #[cfg(test)]
    if let Some(prefix) = &_options.run_prefix {
        return prefix.clone();
    }

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let counter = SUPERVISOR_RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("supervisor-{millis}-{}-{counter}", std::process::id())
}

fn supervisor_worker_id(run_prefix: &str, worker_number: usize) -> String {
    format!("{run_prefix}-worker-{worker_number}")
}

fn started_worker_message(worker_id: &str) -> String {
    format!("started worker {worker_id}")
}

fn supervisor_status_dir(run_prefix: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("tasker-{run_prefix}"));
    fs::create_dir_all(&dir).with_context(|| {
        format!(
            "failed to create supervisor status directory {}",
            dir.display()
        )
    })?;
    Ok(dir)
}

async fn print_progress(pool: &SqlitePool, queue: &str) -> Result<()> {
    let active_runs = tasker_db::active_agent_runs_for_status(pool).await?;
    let holds = tasker_db::active_retry_holds_for_status(pool).await?;
    for run in active_runs.iter().filter(|run| run.queue_key == queue) {
        println!(
            "active Agent Run {} Task {} worker={} lease_expires_at={}",
            run.agent_run_id, run.task_identifier, run.worker_id, run.lease_expires_at
        );
    }
    for hold in holds.iter().filter(|hold| hold.queue_key == queue) {
        println!(
            "active Retry Hold Task {} hold_until={} reason={}",
            hold.task_identifier, hold.hold_until, hold.reason
        );
    }
    Ok(())
}

#[derive(Debug)]
struct UnblockingState {
    unclaimed_eligible: Vec<EligibleTask>,
    active_runs: usize,
    has_only_reported_work: bool,
}

impl UnblockingState {
    fn should_stop(&self) -> bool {
        self.active_runs == 0 && (self.unclaimed_eligible.is_empty() || self.has_only_reported_work)
    }

    fn reason(&self) -> &'static str {
        if self.unclaimed_eligible.is_empty() {
            "drained queue"
        } else {
            "completed handoffs/blocked reports are the only unblocked work"
        }
    }
}

#[derive(Debug, FromRow)]
struct EligibleTask {
    identifier: String,
    state: String,
}

async fn unblocking_state(
    pool: &SqlitePool,
    queue: &str,
    reports: &SupervisorReports,
) -> Result<UnblockingState> {
    let active_runs = tasker_db::active_agent_runs_for_status(pool)
        .await?
        .into_iter()
        .filter(|run| run.queue_key == queue)
        .count();
    let unclaimed_eligible = sqlx::query_as::<_, EligibleTask>(
        r#"
        SELECT tasks.identifier AS identifier, tasks.state AS state
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE task_queues.key = ?
          AND tasks.state IN ('ready', 'in_progress', 'rework', 'integrating')
          AND NOT EXISTS (
              SELECT 1 FROM agent_runs
              WHERE agent_runs.task_id = tasks.id AND agent_runs.outcome IS NULL
          )
          AND NOT EXISTS (
              SELECT 1 FROM task_retry_holds
              WHERE task_retry_holds.task_id = tasks.id AND task_retry_holds.hold_until > CURRENT_TIMESTAMP
          )
        ORDER BY tasks.identifier
        "#,
    )
    .bind(queue)
    .fetch_all(pool)
    .await
    .context("failed to load supervisor unblocking Task state")?;
    let has_only_reported_work = !unclaimed_eligible.is_empty()
        && unclaimed_eligible
            .iter()
            .all(|task| reports.reported_tasks.contains(&task.identifier));
    if has_only_reported_work {
        for task in &unclaimed_eligible {
            println!(
                "unblocked Task {} is reported by Worker Agent and left in {}",
                task.identifier, task.state
            );
        }
    }
    Ok(UnblockingState {
        unclaimed_eligible,
        active_runs,
        has_only_reported_work,
    })
}

async fn active_runs_for_worker(
    pool: &SqlitePool,
    queue: &str,
    worker_id: &str,
) -> Result<Vec<tasker_db::ActiveAgentRunStatus>> {
    Ok(tasker_db::active_agent_runs_for_status(pool)
        .await?
        .into_iter()
        .filter(|run| run.queue_key == queue && run.worker_id == worker_id)
        .collect())
}

fn concise_tail(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut lines: Vec<&str> = trimmed.lines().rev().take(3).collect();
    lines.reverse();
    format!("\n{}", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn supervisor_reports_no_eligible_worker_exit() {
        let temp = TempDir::new().expect("tempdir");
        let script = temp.path().join("worker.sh");
        fs::write(
            &script,
            "#!/bin/sh\necho 'no eligible Tasks found for Task Queue TASK'\nexit 0\n",
        )
        .expect("write script");
        make_executable(&script);
        let pool = empty_pool(temp.path()).await;

        let outcome = supervise_batch(
            &pool,
            SupervisorOptions {
                queue: "TASK".to_string(),
                concurrency: 2,
                timeout_seconds: 2,
                poll_seconds: 1,
                worker_command: vec![script.display().to_string()],
                run_prefix: Some("supervisor-test-no-eligible".to_string()),
            },
        )
        .await
        .expect("supervise");

        assert!(outcome.no_eligible_exits >= 1);
        assert!(outcome.started_workers <= 2);
        assert!(!outcome.timed_out);
    }

    #[tokio::test]
    async fn supervisor_stops_when_worker_reports_completed_handoff() {
        let temp = TempDir::new().expect("tempdir");
        let script = temp.path().join("worker.sh");
        fs::write(
            &script,
            "#!/bin/sh\necho '{\"tasker_worker_status\":true,\"task_identifier\":\"TASK-1\",\"agent_run_id\":\"run-1\",\"status\":\"completion_intent\",\"message\":\"handoff\"}' >> \"$TASKER_WORKER_STATUS_PATH\"\nexit 0\n",
        )
        .expect("write script");
        make_executable(&script);
        let pool = empty_pool(temp.path()).await;
        seed_eligible_integrating_task(&pool).await;

        let outcome = supervise_batch(
            &pool,
            SupervisorOptions {
                queue: "TASK".to_string(),
                concurrency: 1,
                timeout_seconds: 2,
                poll_seconds: 1,
                worker_command: vec![script.display().to_string()],
                run_prefix: Some("supervisor-test-handoff".to_string()),
            },
        )
        .await
        .expect("supervise");

        assert_eq!(outcome.completed_handoffs, 1);
        assert_eq!(outcome.started_workers, 1);
        assert!(!outcome.timed_out);
    }

    #[tokio::test]
    async fn supervisor_reports_stuck_run_after_worker_exit() {
        let temp = TempDir::new().expect("tempdir");
        let script = temp.path().join("worker.sh");
        fs::write(&script, "#!/bin/sh\nexit 1\n").expect("write script");
        make_executable(&script);
        let pool = empty_pool(temp.path()).await;
        seed_active_run(&pool, "supervisor-test-stuck-worker-1").await;

        let outcome = supervise_batch(
            &pool,
            SupervisorOptions {
                queue: "TASK".to_string(),
                concurrency: 1,
                timeout_seconds: 2,
                poll_seconds: 1,
                worker_command: vec![script.display().to_string()],
                run_prefix: Some("supervisor-test-stuck".to_string()),
            },
        )
        .await
        .expect("supervise");

        assert_eq!(outcome.failed_workers, 1);
        assert_eq!(outcome.stuck_runs.len(), 1);
        assert_eq!(outcome.stuck_runs[0].agent_run_id, "run-1");
    }

    #[test]
    fn supervisor_run_prefixes_are_unique_and_worker_ids_use_prefix() {
        let first = supervisor_run_prefix(&SupervisorOptions {
            queue: "TASK".to_string(),
            concurrency: 1,
            timeout_seconds: 1,
            poll_seconds: 1,
            worker_command: vec!["worker".to_string()],
            run_prefix: None,
        });
        let second = supervisor_run_prefix(&SupervisorOptions {
            queue: "TASK".to_string(),
            concurrency: 1,
            timeout_seconds: 1,
            poll_seconds: 1,
            worker_command: vec!["worker".to_string()],
            run_prefix: None,
        });

        assert_ne!(first, second);
        assert_ne!(supervisor_worker_id(&first, 1), "supervisor-worker-1");
        assert_eq!(
            supervisor_worker_id("supervisor-test", 2),
            "supervisor-test-worker-2"
        );
        assert_eq!(
            started_worker_message(&supervisor_worker_id("supervisor-test", 2)),
            "started worker supervisor-test-worker-2"
        );
    }

    async fn empty_pool(path: &std::path::Path) -> SqlitePool {
        let db_path = path.join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        pool
    }

    async fn seed_active_run(pool: &SqlitePool, worker_id: &str) {
        seed_task(pool, "in_progress").await;
        sqlx::query(
            "INSERT INTO agent_runs (id, task_id, task_queue_id, worker_actor_kind, worker_actor_id, worker_actor_display_name, worker_id, launcher_kind, lease_expires_at) VALUES ('run-1', 'task-1', 'queue-1', 'worker_agent', ?, ?, ?, 'fake', datetime('now', '+60 seconds'))",
        )
        .bind(worker_id)
        .bind(worker_id)
        .bind(worker_id)
        .execute(pool)
        .await
        .expect("run");
    }

    async fn seed_eligible_integrating_task(pool: &SqlitePool) {
        seed_task(pool, "integrating").await;
    }

    async fn seed_task(pool: &SqlitePool, state: &str) {
        sqlx::query(
            "INSERT INTO task_queues (id, key, name, delivery_backend, managed_source_repository, main_branch, worktree_root, branch_template, done_worktree_retention) VALUES ('queue-1', 'TASK', 'Tasker', 'local_worktree', '/repo', 'main', '/worktrees', 'tasker/{identifier}', false)",
        )
        .execute(pool)
        .await
        .expect("queue");
        sqlx::query(
            "INSERT INTO tasks (id, task_queue_id, identifier, sequence, title, brief, priority, state, review_required) VALUES ('task-1', 'queue-1', 'TASK-1', 1, 'Test', 'Brief', 'normal', ?, false)",
        )
        .bind(state)
        .execute(pool)
        .await
        .expect("task");
    }

    #[cfg(unix)]
    fn make_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod");
    }
}

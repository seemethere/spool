use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tasker_config::{ensure_data_dir, PathOverrides, TaskerConfig, TaskerPaths};

mod bootstrap;
mod output;
mod supervisor;
mod worker;

#[derive(Debug, Parser)]
#[command(name = "tasker")]
#[command(about = "Local-first task backend for agent-driven development")]
#[command(version)]
struct Cli {
    /// Override the Tasker config file path.
    #[arg(long, global = true, env = "TASKER_CONFIG")]
    config: Option<PathBuf>,

    /// Override the Tasker data directory.
    #[arg(long, global = true, env = "TASKER_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Override the Tasker SQLite database path.
    #[arg(long, global = true, env = "TASKER_DB_PATH")]
    db_path: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize Tasker local config, data directory, and database.
    Init,
    /// Manage Task Queues.
    Queue {
        #[command(subcommand)]
        command: QueueCommand,
    },
    /// Manage Tasks.
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// Show Tasker queue and Task State counts.
    Status,
    /// Run a Worker Loop.
    Work {
        /// Task Queue Key to claim from.
        #[arg(long)]
        queue: String,
        /// Claim and run at most one Task.
        #[arg(long)]
        once: bool,
        /// Agent Launcher to use.
        #[arg(long, default_value = "fake")]
        launcher: String,
        /// Worker Agent actor display name.
        #[arg(long, default_value = "local-worker")]
        actor: String,
        /// Fake Agent Launcher outcome.
        #[arg(long, default_value = "completed")]
        fake_outcome: String,
        /// Claim Lease duration in seconds.
        #[arg(long, default_value_t = 90)]
        lease_seconds: i64,
        /// Retry Hold duration in seconds for failed runs.
        #[arg(long)]
        retry_hold_seconds: Option<i64>,
        /// Maximum launcher execution duration in seconds before failing the Agent Run.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        max_run_seconds: Option<u64>,
        /// Tasker API URL exposed to launched pi sessions.
        #[arg(long)]
        api_url: Option<String>,
        /// Pi executable path.
        #[arg(long, default_value = "pi")]
        pi_bin: String,
        /// Tasker Pi Extension file to load into pi.
        #[arg(long)]
        pi_extension: Option<PathBuf>,
        /// Worker Role Prompt file to send to pi RPC stdin.
        #[arg(long)]
        worker_prompt: Option<PathBuf>,
    },
    /// Supervise a dogfood batch of tasker work --once workers.
    Supervise {
        /// Task Queue Key to supervise.
        #[arg(long)]
        queue: String,
        /// Maximum number of concurrent tasker work --once processes.
        #[arg(long, default_value_t = 1)]
        concurrency: usize,
        /// Exit after this many seconds even if the batch has not drained.
        #[arg(long, default_value_t = 3600)]
        timeout_seconds: u64,
        /// Poll interval for Agent Run and Retry Hold state.
        #[arg(long, default_value_t = 5)]
        poll_seconds: u64,
        /// Agent Launcher passed to default tasker work --once workers.
        #[arg(long, default_value = "pi")]
        launcher: String,
        /// Worker command prefix for tests/debugging; --actor is appended automatically.
        #[arg(long, value_delimiter = ' ')]
        worker_command: Option<Vec<String>>,
    },
    /// Inspect Agent Runs.
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
    /// Temporary Manual Dogfood Merge helpers.
    Merge {
        #[command(subcommand)]
        command: MergeCommand,
    },
    /// Start the Tasker Service.
    Serve {
        /// Override the service bind address.
        #[arg(long)]
        bind: Option<SocketAddr>,
    },
    /// Show the Tasker CLI version.
    Version,
}

#[derive(Debug, Subcommand)]
enum QueueCommand {
    /// Create an Operator-managed Task Queue.
    Create {
        /// Short stable Task Queue Key used in Task Identifiers.
        #[arg(long)]
        key: String,
        /// Human-readable Task Queue name.
        #[arg(long)]
        name: String,
        /// Managed Source Repository path for Local Worktree Delivery.
        #[arg(long)]
        managed_source_repository: PathBuf,
        /// Main Branch for Local Worktree Delivery.
        #[arg(long)]
        main_branch: String,
        /// Worktree Root where Local Worktrees are created.
        #[arg(long)]
        worktree_root: PathBuf,
        /// Branch Template used to derive Task Branch names.
        #[arg(long)]
        branch_template: String,
        /// Keep completed Local Worktrees for debugging.
        #[arg(long, default_value_t = false)]
        done_worktree_retention: bool,
        /// Optional Queue Concurrency Limit for active Agent Runs.
        #[arg(long)]
        queue_concurrency_limit: Option<i64>,
        /// Operator actor display name for audit attribution.
        #[arg(long, default_value = "local-operator")]
        actor: String,
    },
    /// Show a Task Queue by key.
    Show { key: String },
    /// Update Operator-managed Task Queue settings.
    Update {
        /// Task Queue Key to update.
        key: String,
        /// Set a positive Queue Concurrency Limit.
        #[arg(long, conflicts_with = "clear_queue_concurrency_limit")]
        queue_concurrency_limit: Option<i64>,
        /// Clear the Queue Concurrency Limit.
        #[arg(long)]
        clear_queue_concurrency_limit: bool,
        /// Operator actor display name for audit attribution.
        #[arg(long, default_value = "local-operator")]
        actor: String,
    },
    /// Show Audit Events for a Task Queue.
    Audit { key: String },
    /// List Task Queues.
    List,
}

#[derive(Debug, Subcommand)]
enum TaskCommand {
    /// Create a Task.
    Create {
        /// Use temporary Bootstrap Task Creation from a Markdown file with YAML front matter.
        #[arg(long)]
        bootstrap: bool,
        /// Task Queue Key for the new Task.
        #[arg(long)]
        queue: String,
        /// Markdown file containing YAML front matter and the Task Brief body.
        #[arg(long)]
        file: PathBuf,
        /// Operator actor display name for audit attribution.
        #[arg(long, default_value = "local-operator")]
        actor: String,
    },
    /// Show a Task by Task Identifier.
    Show { identifier: String },
    /// Retry recovery: move a resolved failed, canceled, or stuck Task back to Ready.
    Retry {
        /// Task Identifier.
        identifier: String,
        /// Explicit operator reason for retry recovery.
        #[arg(long)]
        reason: String,
        /// Operator actor display name for audit attribution.
        #[arg(long, default_value = "local-operator")]
        actor: String,
    },
    /// Request a normal State Transition for a Task.
    Transition {
        /// Task Identifier.
        identifier: String,
        /// Target Task State.
        #[arg(long)]
        to: String,
        /// Actor kind for audit attribution and permission checks.
        #[arg(long, default_value = "operator")]
        actor_kind: String,
        /// Actor display name for audit attribution.
        #[arg(long, default_value = "local-operator")]
        actor: String,
        /// Active Agent Run ID required for Worker Agent transition to Integrating.
        #[arg(long)]
        agent_run_id: Option<String>,
    },
    /// Update Acceptance Criterion status for a Task.
    Criterion {
        #[command(subcommand)]
        command: RequirementCommand,
    },
    /// Update Validation Item status for a Task.
    Validation {
        #[command(subcommand)]
        command: RequirementCommand,
    },
    /// Update the singleton Workpad Note for a Task.
    Workpad {
        #[command(subcommand)]
        command: WorkpadCommand,
    },
    /// Show Audit Events for a Task.
    Audit { identifier: String },
}

#[derive(Debug, Subcommand)]
enum RequirementCommand {
    /// Set the structured status for a requirement.
    Set {
        /// Task Identifier.
        identifier: String,
        /// 1-based requirement position on the Task.
        #[arg(long)]
        position: i64,
        /// New status, such as satisfied, passed, failed, pending, or waived.
        #[arg(long)]
        status: String,
        /// Explicit reason required when setting waived.
        #[arg(long)]
        waiver_reason: Option<String>,
        /// Operator actor display name for audit attribution.
        #[arg(long, default_value = "local-operator")]
        actor: String,
    },
}

#[derive(Debug, Subcommand)]
enum WorkpadCommand {
    /// Set the current Workpad Note body from a file.
    Set {
        /// Task Identifier.
        identifier: String,
        /// Markdown file containing the Workpad Note body.
        #[arg(long)]
        file: PathBuf,
        /// Operator actor display name for audit attribution.
        #[arg(long, default_value = "local-operator")]
        actor: String,
    },
}

#[derive(Debug, Subcommand)]
enum RunCommand {
    /// Show one Agent Run, its Task, and Launcher Session Data.
    Show { run_id: String },
    /// Operator recovery: fail an active Agent Run and create a Retry Hold.
    Fail {
        /// Agent Run ID.
        run_id: String,
        /// Explicit reason recorded on the Agent Run and Retry Hold.
        #[arg(long)]
        reason: String,
        /// Retry Hold duration in seconds.
        #[arg(long)]
        retry_hold_seconds: Option<i64>,
        /// Operator actor display name for audit attribution.
        #[arg(long, default_value = "local-operator")]
        actor: String,
    },
}

#[derive(Debug, Subcommand)]
enum MergeCommand {
    /// Print a temporary Manual Dogfood Merge inspection plan for a Task.
    Inspect { identifier: String },
    /// Mark a manually merged Integrating Task Done after explicit confirmation.
    Done {
        /// Task Identifier.
        identifier: String,
        /// Confirm that the operator already performed the Local Merge outside Tasker.
        #[arg(long)]
        manual: bool,
        /// Operator actor display name for audit attribution.
        #[arg(long, default_value = "local-operator")]
        actor: String,
    },
}

struct WorkOptions {
    queue: String,
    once: bool,
    launcher: String,
    actor: String,
    fake_outcome: String,
    lease_seconds: i64,
    retry_hold_seconds: Option<i64>,
    max_run_seconds: Option<u64>,
    api_url: Option<String>,
    pi_bin: String,
    pi_extension: Option<PathBuf>,
    worker_prompt: Option<PathBuf>,
}

struct SuperviseOptions {
    queue: String,
    concurrency: usize,
    timeout_seconds: u64,
    poll_seconds: u64,
    launcher: String,
    worker_command: Option<Vec<String>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let paths = cli.paths()?;
    let db_path_overridden = cli.db_path.is_some();
    guard_project_config(&cli, &paths, db_path_overridden)?;

    match cli.command {
        Some(Command::Init) => init(&paths, db_path_overridden).await,
        Some(Command::Queue { command }) => queue(&paths, db_path_overridden, command).await,
        Some(Command::Task { command }) => task(&paths, db_path_overridden, command).await,
        Some(Command::Status) => status(&paths, db_path_overridden).await,
        Some(Command::Work {
            queue,
            once,
            launcher,
            actor,
            fake_outcome,
            lease_seconds,
            retry_hold_seconds,
            max_run_seconds,
            api_url,
            pi_bin,
            pi_extension,
            worker_prompt,
        }) => {
            work(
                &paths,
                db_path_overridden,
                WorkOptions {
                    queue,
                    once,
                    launcher,
                    actor,
                    fake_outcome,
                    lease_seconds,
                    retry_hold_seconds,
                    max_run_seconds,
                    api_url,
                    pi_bin,
                    pi_extension,
                    worker_prompt,
                },
            )
            .await
        }
        Some(Command::Supervise {
            queue,
            concurrency,
            timeout_seconds,
            poll_seconds,
            launcher,
            worker_command,
        }) => {
            supervise(
                &paths,
                db_path_overridden,
                SuperviseOptions {
                    queue,
                    concurrency,
                    timeout_seconds,
                    poll_seconds,
                    launcher,
                    worker_command,
                },
            )
            .await
        }
        Some(Command::Run { command }) => run(&paths, db_path_overridden, command).await,
        Some(Command::Merge { command }) => merge(&paths, db_path_overridden, command).await,
        Some(Command::Serve { bind }) => serve(&paths, bind, db_path_overridden).await,
        Some(Command::Version) => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        None => {
            println!("Tasker CLI skeleton. Run `tasker --help` for usage.");
            Ok(())
        }
    }
}

impl Cli {
    fn paths(&self) -> Result<TaskerPaths> {
        TaskerPaths::from_env(PathOverrides {
            config_path: self.config.clone(),
            data_dir: self.data_dir.clone(),
            db_path: self.db_path.clone(),
        })
    }

    fn has_intentional_config_override(&self) -> bool {
        self.config.is_some() || self.data_dir.is_some() || self.db_path.is_some()
    }
}

fn guard_project_config(cli: &Cli, paths: &TaskerPaths, db_path_overridden: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
    guard_project_config_from(cli, paths, db_path_overridden, &cwd)
}

fn guard_project_config_from(
    cli: &Cli,
    paths: &TaskerPaths,
    db_path_overridden: bool,
    cwd: &Path,
) -> Result<()> {
    let Some(project_config) = discover_project_config(cwd) else {
        return Ok(());
    };
    if paths_equivalent(&paths.config_path, &project_config) {
        return Ok(());
    }

    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }

    if command_is_unsafe_mutation(&cli.command) && !cli.has_intentional_config_override() {
        anyhow::bail!(
            "refusing mutating Tasker command because project config {} is present but inactive; active config is {} and active database is {}. Re-run with --config .tasker/config.toml, set TASKER_CONFIG, use bin/tasker-local, or pass an intentional --data-dir/--db-path override.",
            project_config.display(),
            paths.config_path.display(),
            config.database.path.display()
        );
    }

    eprintln!(
        "warning: project config {} is present but inactive; active config is {} and active database is {}. Use --config .tasker/config.toml or bin/tasker-local to target this project.",
        project_config.display(),
        paths.config_path.display(),
        config.database.path.display()
    );
    Ok(())
}

fn discover_project_config(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        let candidate = ancestor.join(".tasker/config.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn command_is_unsafe_mutation(command: &Option<Command>) -> bool {
    match command {
        Some(
            Command::Init
            | Command::Work { .. }
            | Command::Supervise { .. }
            | Command::Serve { .. },
        ) => true,
        Some(Command::Queue { command }) => matches!(
            command,
            QueueCommand::Create { .. } | QueueCommand::Update { .. }
        ),
        Some(Command::Task { command }) => !matches!(
            command,
            TaskCommand::Show { .. } | TaskCommand::Audit { .. }
        ),
        Some(Command::Run { command }) => matches!(command, RunCommand::Fail { .. }),
        Some(Command::Merge { command }) => matches!(command, MergeCommand::Done { .. }),
        Some(Command::Status | Command::Version) | None => false,
    }
}

async fn init(paths: &TaskerPaths, db_path_overridden: bool) -> Result<()> {
    ensure_data_dir(paths)?;

    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    let wrote_config = config.write_if_missing(paths)?;
    ensure_db_parent(&config.database.path)?;

    let pool = tasker_db::connect(&config.database.path).await?;
    tasker_db::run_migrations(&pool).await?;
    let token = tasker_db::ensure_local_api_token(&pool).await?;

    println!("Tasker initialized");
    println!("config: {}", paths.config_path.display());
    println!("data: {}", paths.data_dir.display());
    println!("database: {}", config.database.path.display());
    println!("local api token: {token}");
    if !wrote_config {
        println!("config already existed; left unchanged");
    }

    Ok(())
}

async fn queue(paths: &TaskerPaths, db_path_overridden: bool, command: QueueCommand) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;

    match command {
        QueueCommand::Create {
            key,
            name,
            managed_source_repository,
            main_branch,
            worktree_root,
            branch_template,
            done_worktree_retention,
            queue_concurrency_limit,
            actor,
        } => {
            println!(
                "warning: Local Worktree Delivery may mutate Managed Source Repository {}",
                managed_source_repository.display()
            );
            let input = tasker_db::CreateTaskQueue {
                key,
                name,
                managed_source_repository: managed_source_repository.display().to_string(),
                main_branch,
                worktree_root: worktree_root.display().to_string(),
                branch_template,
                done_worktree_retention,
                queue_concurrency_limit,
            };
            let queue =
                tasker_db::create_task_queue(&pool, &input, &tasker_db::Actor::operator(actor))
                    .await?;
            output::print_queue(&queue)?;
        }
        QueueCommand::Show { key } => {
            let queue = tasker_db::get_task_queue(&pool, &key)
                .await?
                .with_context(|| format!("Task Queue {key} not found"))?;
            output::print_queue(&queue)?;
        }
        QueueCommand::Update {
            key,
            queue_concurrency_limit,
            clear_queue_concurrency_limit,
            actor,
        } => {
            if queue_concurrency_limit.is_none() && !clear_queue_concurrency_limit {
                anyhow::bail!(
                    "queue update requires --queue-concurrency-limit or --clear-queue-concurrency-limit"
                );
            }
            let limit = if clear_queue_concurrency_limit {
                None
            } else {
                queue_concurrency_limit
            };
            let queue = tasker_db::update_task_queue_concurrency_limit(
                &pool,
                &key,
                &tasker_db::UpdateQueueConcurrencyLimit {
                    queue_concurrency_limit: limit,
                },
                &tasker_db::Actor::operator(actor),
            )
            .await?;
            output::print_queue(&queue)?;
        }
        QueueCommand::Audit { key } => {
            let events = tasker_db::list_task_queue_audit_events(&pool, &key).await?;
            for event in events {
                println!(
                    "{}\t{}\t{} ({})\t{}",
                    event.created_at,
                    event.event_type,
                    event.actor_display_name,
                    event.actor_kind,
                    event.payload_json
                );
            }
        }
        QueueCommand::List => {
            let queues = tasker_db::list_task_queues(&pool).await?;
            for queue in queues {
                println!("{}\t{}", queue.key, queue.name);
            }
        }
    }

    Ok(())
}

async fn task(paths: &TaskerPaths, db_path_overridden: bool, command: TaskCommand) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;

    match command {
        TaskCommand::Create {
            bootstrap,
            queue,
            file,
            actor,
        } => {
            if !bootstrap {
                anyhow::bail!("task create currently requires --bootstrap");
            }
            let input = bootstrap::parse_bootstrap_task_file(&queue, &file)?;
            let detail =
                tasker_db::create_task(&pool, &input, &tasker_db::Actor::operator(actor)).await?;
            println!("created Task: {}", detail.task.identifier);
            println!("title: {}", detail.task.title);
            println!("state: {}", detail.task.state);
        }
        TaskCommand::Show { identifier } => {
            let detail = tasker_db::get_task_detail(&pool, &identifier)
                .await?
                .with_context(|| format!("Task {identifier} not found"))?;
            output::print_task_detail(&detail)?;
        }
        TaskCommand::Retry {
            identifier,
            reason,
            actor,
        } => {
            let detail = tasker_db::retry_task(
                &pool,
                &identifier,
                &tasker_db::RetryTaskInput { reason },
                &tasker_db::Actor::operator(actor),
            )
            .await?;
            println!("retried Task {} to Ready", detail.task.identifier);
        }
        TaskCommand::Transition {
            identifier,
            to,
            actor_kind,
            actor,
            agent_run_id,
        } => {
            let detail = tasker_db::transition_task_state(
                &pool,
                &identifier,
                &tasker_db::TransitionTaskState {
                    to_state: bootstrap::normalize_label(&to),
                    agent_run_id,
                },
                &tasker_db::Actor {
                    kind: actor_kind,
                    id: actor.clone(),
                    display_name: actor,
                },
            )
            .await?;
            println!(
                "transitioned Task {} to {}",
                detail.task.identifier, detail.task.state
            );
        }
        TaskCommand::Criterion { command } => match command {
            RequirementCommand::Set {
                identifier,
                position,
                status,
                waiver_reason,
                actor,
            } => {
                let input = tasker_db::UpdateRequirementStatus {
                    status: bootstrap::normalize_label(&status),
                    waiver_reason,
                };
                let detail = tasker_db::update_acceptance_criterion_status(
                    &pool,
                    &identifier,
                    position,
                    &input,
                    &tasker_db::Actor::operator(actor),
                )
                .await?;
                println!(
                    "updated Acceptance Criterion {position} for Task {}",
                    detail.task.identifier
                );
            }
        },
        TaskCommand::Validation { command } => match command {
            RequirementCommand::Set {
                identifier,
                position,
                status,
                waiver_reason,
                actor,
            } => {
                let input = tasker_db::UpdateRequirementStatus {
                    status: bootstrap::normalize_label(&status),
                    waiver_reason,
                };
                let detail = tasker_db::update_validation_item_status(
                    &pool,
                    &identifier,
                    position,
                    &input,
                    &tasker_db::Actor::operator(actor),
                )
                .await?;
                println!(
                    "updated Validation Item {position} for Task {}",
                    detail.task.identifier
                );
            }
        },
        TaskCommand::Workpad { command } => match command {
            WorkpadCommand::Set {
                identifier,
                file,
                actor,
            } => {
                let body = fs::read_to_string(&file)
                    .with_context(|| format!("failed to read {}", file.display()))?;
                let detail = tasker_db::update_workpad_note(
                    &pool,
                    &identifier,
                    &body,
                    &tasker_db::Actor::operator(actor),
                )
                .await?;
                println!("updated Workpad Note for Task {}", detail.task.identifier);
            }
        },
        TaskCommand::Audit { identifier } => {
            let events = tasker_db::list_task_audit_events(&pool, &identifier).await?;
            for event in events {
                println!(
                    "{}\t{}\t{} ({})\t{}",
                    event.created_at,
                    event.event_type,
                    event.actor_display_name,
                    event.actor_kind,
                    event.payload_json
                );
            }
        }
    }

    Ok(())
}

async fn status(paths: &TaskerPaths, db_path_overridden: bool) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;
    let rows = tasker_db::status_by_queue_and_state(&pool).await?;
    if rows.is_empty() {
        println!("No Task Queues found");
        return Ok(());
    }

    let active_runs = tasker_db::active_agent_runs_for_status(&pool).await?;
    let active_holds = tasker_db::active_retry_holds_for_status(&pool).await?;

    let mut current_queue: Option<String> = None;
    for row in rows {
        let queue_header = format!("{}\t{}", row.queue_key, row.queue_name);
        if current_queue.as_ref() != Some(&queue_header) {
            if current_queue.is_some() {
                println!();
            }
            println!("Task Queue: {queue_header}");
            println!("  active Agent Runs: {}", row.active_agent_runs);
            for run in active_runs
                .iter()
                .filter(|run| run.queue_key.as_str() == row.queue_key.as_str())
            {
                println!(
                    "    {}\t{}\tlauncher={}\tworker={}\tlease_expires_at={}",
                    run.task_identifier,
                    run.agent_run_id,
                    run.launcher_kind,
                    run.worker_id,
                    run.lease_expires_at
                );
            }
            println!("  active Retry Holds: {}", row.active_retry_holds);
            for hold in active_holds
                .iter()
                .filter(|hold| hold.queue_key.as_str() == row.queue_key.as_str())
            {
                println!(
                    "    {}\thold_until={}\treason={}",
                    hold.task_identifier, hold.hold_until, hold.reason
                );
            }
            current_queue = Some(queue_header);
        }
        println!("  {}: {}", row.state, row.task_count);
    }

    Ok(())
}

async fn work(paths: &TaskerPaths, db_path_overridden: bool, options: WorkOptions) -> Result<()> {
    if !options.once {
        anyhow::bail!("tasker work currently requires --once");
    }
    let pool = open_pool(paths, db_path_overridden).await?;
    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    let api_token = tasker_db::ensure_local_api_token(&pool).await?;
    let api_url = options
        .api_url
        .unwrap_or_else(|| format!("http://{}", config.service.bind_addr));
    let outcome = worker::run_worker_once(
        &pool,
        worker::WorkOnceRequest {
            queue: options.queue,
            launcher: options.launcher,
            actor: options.actor,
            fake_outcome: options.fake_outcome,
            lease_seconds: options.lease_seconds,
            retry_hold_seconds: options.retry_hold_seconds,
            max_run_seconds: options.max_run_seconds,
            data_dir: paths.data_dir.clone(),
            api_url,
            api_token,
            pi_bin: options.pi_bin,
            pi_extension: options.pi_extension,
            worker_prompt: options.worker_prompt,
        },
    )
    .await?;

    match outcome {
        worker::WorkOnceOutcome::NoEligibleTask { queue } => {
            println!("no eligible Tasks found for Task Queue {queue}");
        }
        worker::WorkOnceOutcome::Finished {
            task_identifier,
            run_id,
            outcome,
        } => {
            println!("claimed Task {task_identifier} with Agent Run {run_id}");
            println!("finished Agent Run {run_id} with outcome {outcome}");
        }
    }
    Ok(())
}

async fn supervise(
    paths: &TaskerPaths,
    db_path_overridden: bool,
    options: SuperviseOptions,
) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;
    let command = if let Some(command) = options.worker_command {
        command
    } else {
        let exe = std::env::current_exe().context("failed to resolve current tasker executable")?;
        vec![
            exe.display().to_string(),
            "--config".to_string(),
            paths.config_path.display().to_string(),
            "--data-dir".to_string(),
            paths.data_dir.display().to_string(),
            "--db-path".to_string(),
            paths.db_path.display().to_string(),
            "work".to_string(),
            "--once".to_string(),
            "--queue".to_string(),
            options.queue.clone(),
            "--launcher".to_string(),
            options.launcher,
        ]
    };

    let outcome = supervisor::supervise_batch(
        &pool,
        supervisor::SupervisorOptions {
            queue: options.queue,
            concurrency: options.concurrency,
            timeout_seconds: options.timeout_seconds,
            poll_seconds: options.poll_seconds,
            worker_command: command,
        },
    )
    .await?;

    println!(
        "supervisor summary: started={} completed={} failed={} no_eligible={} completed_handoffs={} blocked_reports={} retryable_failures={} stuck_runs={} timed_out={}",
        outcome.started_workers,
        outcome.completed_workers,
        outcome.failed_workers,
        outcome.no_eligible_exits,
        outcome.completed_handoffs,
        outcome.blocked_reports,
        outcome.retryable_failure_reports,
        outcome.stuck_runs.len(),
        outcome.timed_out
    );
    Ok(())
}

async fn run(paths: &TaskerPaths, db_path_overridden: bool, command: RunCommand) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;
    match command {
        RunCommand::Show { run_id } => {
            let detail = tasker_db::get_agent_run_detail(&pool, &run_id)
                .await?
                .with_context(|| format!("Agent Run {run_id} not found"))?;
            output::print_run_detail(&detail)?;
        }
        RunCommand::Fail {
            run_id,
            reason,
            retry_hold_seconds,
            actor,
        } => {
            let run = tasker_db::operator_fail_run(
                &pool,
                &run_id,
                &tasker_db::OperatorFailRunInput {
                    failure_reason: reason,
                    retry_hold_seconds,
                },
                &tasker_db::Actor::operator(actor),
            )
            .await?;
            let detail = tasker_db::get_agent_run_detail(&pool, &run.id)
                .await?
                .with_context(|| format!("Agent Run {} not found after failure", run.id))?;
            println!("failed Agent Run {}", detail.run.id);
            println!(
                "retry hold created for Task {}",
                detail.task.task.identifier
            );
        }
    }
    Ok(())
}

async fn merge(paths: &TaskerPaths, db_path_overridden: bool, command: MergeCommand) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;
    match command {
        MergeCommand::Inspect { identifier } => {
            let detail = tasker_db::get_task_detail(&pool, &identifier)
                .await?
                .with_context(|| format!("Task {identifier} not found"))?;
            let queue = tasker_db::get_task_queue(&pool, &detail.task.task_queue_key)
                .await?
                .with_context(|| format!("Task Queue {} not found", detail.task.task_queue_key))?;
            let latest_run =
                tasker_db::get_latest_agent_run_detail_for_task(&pool, &identifier).await?;
            print_manual_merge_inspection(&detail, &queue, latest_run.as_ref());
        }
        MergeCommand::Done {
            identifier,
            manual,
            actor,
        } => {
            if !manual {
                anyhow::bail!(
                    "refusing to mark Task Done without --manual confirmation that the Local Merge was performed outside Tasker"
                );
            }
            let current = tasker_db::get_task_detail(&pool, &identifier)
                .await?
                .with_context(|| format!("Task {identifier} not found"))?;
            if current.task.state != "integrating" {
                anyhow::bail!(
                    "Manual Dogfood Merge completion requires Task State integrating; current state is {}",
                    current.task.state
                );
            }
            let detail = tasker_db::transition_task_state(
                &pool,
                &identifier,
                &tasker_db::TransitionTaskState {
                    to_state: "done".to_string(),
                    agent_run_id: None,
                },
                &tasker_db::Actor::operator(actor),
            )
            .await?;
            println!(
                "marked manually merged Task {} Done",
                detail.task.identifier
            );
        }
    }
    Ok(())
}

fn print_manual_merge_inspection(
    detail: &tasker_db::TaskDetail,
    queue: &tasker_db::TaskQueue,
    latest_run: Option<&tasker_db::AgentRunDetail>,
) {
    let local_worktree = detail
        .task_links
        .iter()
        .find(|link| link.kind == "local_worktree")
        .map(|link| link.target.as_str());
    let task_branch = detail
        .task_links
        .iter()
        .find(|link| link.kind == "task_branch")
        .map(|link| link.target.as_str());

    println!("Manual Dogfood Merge inspection plan");
    println!("temporary helper: Git operations remain operator-side; Tasker Service performs no Git mutations");
    println!();
    println!("Task: {}", detail.task.identifier);
    println!("title: {}", detail.task.title);
    println!("Task State: {}", detail.task.state);
    println!("Task Queue: {}", detail.task.task_queue_key);
    println!(
        "Managed Source Repository: {}",
        queue.managed_source_repository
    );
    println!("Main Branch: {}", queue.main_branch);
    println!(
        "Local Worktree: {}",
        local_worktree.unwrap_or("missing Task Link")
    );
    println!(
        "Task Branch: {}",
        task_branch.unwrap_or("missing Task Link")
    );
    println!(
        "Workpad Note: {}",
        if detail.workpad_note.is_some() {
            "present"
        } else {
            "missing"
        }
    );
    println!();
    print_local_worktree_git_inspection(local_worktree, task_branch, &queue.main_branch);
    println!();
    println!("Latest Agent Run:");
    if let Some(run) = latest_run {
        println!("  id: {}", run.run.id);
        println!("  launcher: {}", run.run.launcher_kind);
        println!(
            "  outcome: {}",
            run.run.outcome.as_deref().unwrap_or("active")
        );
        if let Some(reason) = &run.run.failure_reason {
            println!("  failure reason: {reason}");
        }
        if let Some(session) = &run.launcher_session_data {
            println!(
                "  Run Transcript: {}",
                session.transcript_path.as_deref().unwrap_or("not recorded")
            );
            println!(
                "  Launcher Session Data: present{}",
                session
                    .final_status
                    .as_deref()
                    .map(|status| format!(" (final status: {status})"))
                    .unwrap_or_default()
            );
        } else {
            println!("  Run Transcript: not recorded");
            println!("  Launcher Session Data: missing");
        }
    } else {
        println!("  (none)");
        println!("  Run Transcript: not recorded");
        println!("  Launcher Session Data: missing");
    }
    println!();
    println!("Structured gates:");
    for criterion in &detail.acceptance_criteria {
        println!(
            "  Acceptance Criterion {}: [{}] {}",
            criterion.position, criterion.status, criterion.description
        );
    }
    for item in &detail.validation_items {
        println!(
            "  Validation Item {}: [{}] {}",
            item.position, item.status, item.description
        );
    }
    println!();
    println!("Suggested validation commands:");
    println!("  cargo test");
    println!("  cargo clippy --all-targets --all-features -- -D warnings");
    println!("  if TypeScript extension files changed: (cd extensions/tasker-pi && bun test && bun run build)");
    println!();
    println!("Operator-side checklist:");
    println!("  1. Inspect Tasker state, latest Agent Run, Run Transcript, and Workpad Note.");
    println!("  2. From the Local Worktree, verify a clean working tree and focused Task Commits.");
    println!("  3. Run current validation from the Local Worktree after any refresh.");
    println!("  4. Perform the Local Merge into the Main Branch outside Tasker.");
    println!(
        "  5. After the manual merge, run: tasker merge done {} --manual",
        detail.task.identifier
    );
}

fn print_local_worktree_git_inspection(
    local_worktree: Option<&str>,
    task_branch: Option<&str>,
    main_branch: &str,
) {
    println!("Local Worktree Git inspection (read-only):");
    println!("  Git mutations: none; commands below are inspection-only");
    let Some(local_worktree) = local_worktree else {
        println!("  clean: unknown (missing Local Worktree Task Link)");
        println!("  diff from Main Branch: unavailable");
        return;
    };

    let worktree = Path::new(local_worktree);
    if !worktree.exists() {
        println!("  clean: unknown (Local Worktree path does not exist)");
        println!("  diff from Main Branch: unavailable");
        return;
    }

    match git_output(worktree, &["status", "--porcelain"]) {
        Ok(status) if status.trim().is_empty() => println!("  clean: yes"),
        Ok(status) => {
            println!("  clean: no");
            for line in status.lines() {
                println!("    {line}");
            }
        }
        Err(error) => println!("  clean: unknown ({error})"),
    }

    match git_output(worktree, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Ok(branch) => {
            let branch = branch.trim();
            println!("  checked-out branch: {branch}");
            if let Some(expected) = task_branch {
                if branch != expected {
                    println!("  warning: checked-out branch differs from Task Branch {expected}");
                }
            }
        }
        Err(error) => println!("  checked-out branch: unknown ({error})"),
    }

    let comparison = format!("{main_branch}...HEAD");
    match git_output(worktree, &["diff", "--stat", &comparison]) {
        Ok(stat) if stat.trim().is_empty() => {
            println!("  diff from Main Branch ({comparison}): no file changes")
        }
        Ok(stat) => {
            println!("  diff from Main Branch ({comparison}):");
            for line in stat.lines() {
                println!("    {line}");
            }
        }
        Err(error) => println!("  diff from Main Branch ({comparison}): unavailable ({error})"),
    }

    let commits = format!("{main_branch}..HEAD");
    match git_output(worktree, &["log", "--oneline", &commits]) {
        Ok(log) if log.trim().is_empty() => {
            println!("  Task Commits since Main Branch ({commits}): none")
        }
        Ok(log) => {
            println!("  Task Commits since Main Branch ({commits}):");
            for line in log.lines() {
                println!("    {line}");
            }
        }
        Err(error) => {
            println!("  Task Commits since Main Branch ({commits}): unavailable ({error})")
        }
    }
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

async fn serve(
    paths: &TaskerPaths,
    bind: Option<SocketAddr>,
    db_path_overridden: bool,
) -> Result<()> {
    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    let bind_addr = match bind {
        Some(bind) => bind,
        None => config
            .service
            .bind_addr
            .parse()
            .with_context(|| format!("invalid bind address {}", config.service.bind_addr))?,
    };

    let pool = tasker_db::connect(&config.database.path).await?;
    tasker_db::run_migrations(&pool).await?;

    tasker_server::serve(bind_addr, env!("CARGO_PKG_VERSION"), pool).await
}

async fn open_pool(paths: &TaskerPaths, db_path_overridden: bool) -> Result<sqlx::SqlitePool> {
    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    let pool = tasker_db::connect(&config.database.path).await?;
    tasker_db::run_migrations(&pool).await?;
    Ok(pool)
}

fn ensure_db_parent(db_path: &Path) -> Result<()> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn project_config_guard_refuses_mutating_command_when_inactive() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let nested = repo.join("src");
        fs::create_dir_all(&nested).expect("nested repo dir");
        let project_config = repo.join(".tasker/config.toml");
        write_config(&project_config, &repo.join(".tasker/data/tasker.db"));
        let paths = TaskerPaths::resolve(temp.path().join("home"), PathOverrides::default());
        let cli = Cli {
            config: None,
            data_dir: None,
            db_path: None,
            command: Some(Command::Task {
                command: TaskCommand::Create {
                    bootstrap: true,
                    queue: "TASK".to_string(),
                    file: repo.join("task.md"),
                    actor: "tester".to_string(),
                },
            }),
        };

        let error = guard_project_config_from(&cli, &paths, false, &nested)
            .expect_err("inactive project config should refuse mutation");
        let message = error.to_string();

        assert!(message.contains("refusing mutating Tasker command"));
        assert!(message.contains("--config .tasker/config.toml"));
        assert!(message.contains("bin/tasker-local"));
        assert!(message.contains(&project_config.display().to_string()));
        assert!(message.contains(&paths.config_path.display().to_string()));
        assert!(message.contains(&paths.db_path.display().to_string()));
    }

    #[test]
    fn project_config_guard_allows_explicit_config_for_mutating_command() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let nested = repo.join("src");
        fs::create_dir_all(&nested).expect("nested repo dir");
        let project_config = repo.join(".tasker/config.toml");
        let project_db = repo.join(".tasker/data/tasker.db");
        write_config(&project_config, &project_db);
        let paths = TaskerPaths::resolve(
            temp.path().join("home"),
            PathOverrides {
                config_path: Some(project_config),
                ..PathOverrides::default()
            },
        );
        let cli = Cli {
            config: Some(paths.config_path.clone()),
            data_dir: None,
            db_path: None,
            command: Some(Command::Task {
                command: TaskCommand::Create {
                    bootstrap: true,
                    queue: "TASK".to_string(),
                    file: repo.join("task.md"),
                    actor: "tester".to_string(),
                },
            }),
        };

        guard_project_config_from(&cli, &paths, false, &nested)
            .expect("explicit project config should allow mutation");
    }

    #[test]
    fn project_config_guard_allows_read_only_command_when_inactive() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let nested = repo.join("src");
        fs::create_dir_all(&nested).expect("nested repo dir");
        write_config(
            &repo.join(".tasker/config.toml"),
            &repo.join(".tasker/data/tasker.db"),
        );
        let paths = TaskerPaths::resolve(temp.path().join("home"), PathOverrides::default());
        let cli = Cli {
            config: None,
            data_dir: None,
            db_path: None,
            command: Some(Command::Task {
                command: TaskCommand::Show {
                    identifier: "TASK-1".to_string(),
                },
            }),
        };

        guard_project_config_from(&cli, &paths, false, &nested)
            .expect("read-only command should warn but continue");
    }

    fn write_config(config_path: &Path, db_path: &Path) {
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent).expect("config parent");
        }
        fs::write(
            config_path,
            format!(
                "[service]\nbind_addr = \"{}\"\n\n[database]\npath = \"{}\"\n",
                tasker_config::DEFAULT_BIND_ADDR,
                db_path.display()
            ),
        )
        .expect("write config");
    }

    #[tokio::test]
    async fn init_creates_local_state_and_is_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = TaskerPaths::resolve(temp.path(), PathOverrides::default());

        init(&paths, false).await.expect("first init");
        let config_text = fs::read_to_string(&paths.config_path).expect("config text");
        init(&paths, false).await.expect("second init");

        assert!(paths.data_dir.is_dir());
        assert!(paths.config_path.is_file());
        assert!(paths.db_path.is_file());
        assert_eq!(fs::read_to_string(&paths.config_path).unwrap(), config_text);
    }

    #[tokio::test]
    async fn init_creates_parent_directory_for_custom_db_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = TaskerPaths::resolve(
            temp.path(),
            PathOverrides {
                db_path: Some(temp.path().join("custom/sub/tasker.db")),
                ..PathOverrides::default()
            },
        );

        init(&paths, true).await.expect("init");

        assert!(paths.db_path.is_file());
    }

    #[tokio::test]
    async fn init_uses_existing_config_database_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = TaskerPaths::resolve(temp.path(), PathOverrides::default());
        let configured_db_path = temp.path().join("configured/tasker.db");
        let config = TaskerConfig {
            service: tasker_config::ServiceConfig {
                bind_addr: tasker_config::DEFAULT_BIND_ADDR.to_string(),
            },
            database: tasker_config::DatabaseConfig {
                path: configured_db_path.clone(),
            },
        };
        config.write_if_missing(&paths).expect("write config");

        init(&paths, false).await.expect("init");

        assert!(configured_db_path.is_file());
        assert!(!paths.db_path.exists());
    }

    #[tokio::test]
    async fn queue_commands_create_show_and_list() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = TaskerPaths::resolve(temp.path(), PathOverrides::default());
        init(&paths, false).await.expect("init");

        queue(
            &paths,
            false,
            QueueCommand::Create {
                key: "TASK".to_string(),
                name: "Tasker".to_string(),
                managed_source_repository: temp.path().join("repo"),
                main_branch: "main".to_string(),
                worktree_root: temp.path().join("worktrees"),
                branch_template: "tasker/{task_identifier}".to_string(),
                done_worktree_retention: false,
                queue_concurrency_limit: None,
                actor: "tester".to_string(),
            },
        )
        .await
        .expect("create queue");
        queue(
            &paths,
            false,
            QueueCommand::Show {
                key: "TASK".to_string(),
            },
        )
        .await
        .expect("show queue");
        queue(
            &paths,
            false,
            QueueCommand::Update {
                key: "TASK".to_string(),
                queue_concurrency_limit: Some(2),
                clear_queue_concurrency_limit: false,
                actor: "tester".to_string(),
            },
        )
        .await
        .expect("set Queue Concurrency Limit");
        queue(
            &paths,
            false,
            QueueCommand::Update {
                key: "TASK".to_string(),
                queue_concurrency_limit: None,
                clear_queue_concurrency_limit: true,
                actor: "tester".to_string(),
            },
        )
        .await
        .expect("clear Queue Concurrency Limit");
        queue(
            &paths,
            false,
            QueueCommand::Audit {
                key: "TASK".to_string(),
            },
        )
        .await
        .expect("queue audit");
        queue(&paths, false, QueueCommand::List)
            .await
            .expect("list queues");
    }

    #[tokio::test]
    async fn task_commands_create_show_workpad_status_and_merge() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = TaskerPaths::resolve(temp.path(), PathOverrides::default());
        init(&paths, false).await.expect("init");
        let repo = temp.path().join("repo");
        init_git_repo(&repo);
        queue(
            &paths,
            false,
            QueueCommand::Create {
                key: "TASK".to_string(),
                name: "Tasker".to_string(),
                managed_source_repository: repo,
                main_branch: "main".to_string(),
                worktree_root: temp.path().join("worktrees"),
                branch_template: "tasker/{task_identifier}".to_string(),
                done_worktree_retention: false,
                queue_concurrency_limit: None,
                actor: "tester".to_string(),
            },
        )
        .await
        .expect("create queue");

        let task_file = temp.path().join("task.md");
        fs::write(
            &task_file,
            r#"---
title: Add bootstrap task creation
priority: high
acceptance_criteria:
  - Bootstrap file creates a Task
validation_items:
  - cargo test passes
tags:
  - dogfood
  - backend
---
Implement Bootstrap Task Creation.
"#,
        )
        .expect("write task file");
        task(
            &paths,
            false,
            TaskCommand::Create {
                bootstrap: true,
                queue: "TASK".to_string(),
                file: task_file,
                actor: "tester".to_string(),
            },
        )
        .await
        .expect("create task");
        task(
            &paths,
            false,
            TaskCommand::Show {
                identifier: "TASK-1".to_string(),
            },
        )
        .await
        .expect("show task");

        task(
            &paths,
            false,
            TaskCommand::Criterion {
                command: RequirementCommand::Set {
                    identifier: "TASK-1".to_string(),
                    position: 1,
                    status: "satisfied".to_string(),
                    waiver_reason: None,
                    actor: "tester".to_string(),
                },
            },
        )
        .await
        .expect("set criterion");
        task(
            &paths,
            false,
            TaskCommand::Validation {
                command: RequirementCommand::Set {
                    identifier: "TASK-1".to_string(),
                    position: 1,
                    status: "passed".to_string(),
                    waiver_reason: None,
                    actor: "tester".to_string(),
                },
            },
        )
        .await
        .expect("set validation");

        let workpad_file = temp.path().join("workpad.md");
        fs::write(&workpad_file, "Plan and evidence").expect("write workpad");
        task(
            &paths,
            false,
            TaskCommand::Workpad {
                command: WorkpadCommand::Set {
                    identifier: "TASK-1".to_string(),
                    file: workpad_file,
                    actor: "tester".to_string(),
                },
            },
        )
        .await
        .expect("set workpad");
        task(
            &paths,
            false,
            TaskCommand::Audit {
                identifier: "TASK-1".to_string(),
            },
        )
        .await
        .expect("audit");
        work(
            &paths,
            false,
            WorkOptions {
                queue: "TASK".to_string(),
                once: true,
                launcher: "fake".to_string(),
                actor: "worker".to_string(),
                fake_outcome: "completed".to_string(),
                lease_seconds: 90,
                retry_hold_seconds: None,
                max_run_seconds: None,
                api_url: None,
                pi_bin: "pi".to_string(),
                pi_extension: None,
                worker_prompt: None,
            },
        )
        .await
        .expect("fake work");
        let pool = open_pool(&paths, false).await.expect("pool");
        let missing_run = tasker_db::get_agent_run(&pool, "not-a-real-run")
            .await
            .expect("get missing run");
        assert!(missing_run.is_none());
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("get task")
            .expect("task");
        assert!(detail
            .workpad_note
            .unwrap()
            .body
            .contains("Fake Agent Launcher processed Task TASK-1"));
        task(
            &paths,
            false,
            TaskCommand::Transition {
                identifier: "TASK-1".to_string(),
                to: "integrating".to_string(),
                actor_kind: "operator".to_string(),
                actor: "tester".to_string(),
                agent_run_id: None,
            },
        )
        .await
        .expect("transition");
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("get task")
            .expect("task");
        assert_eq!(detail.task.state, "integrating");
        merge(
            &paths,
            false,
            MergeCommand::Inspect {
                identifier: "TASK-1".to_string(),
            },
        )
        .await
        .expect("inspect manual merge");
        let missing_manual = merge(
            &paths,
            false,
            MergeCommand::Done {
                identifier: "TASK-1".to_string(),
                manual: false,
                actor: "tester".to_string(),
            },
        )
        .await
        .expect_err("manual confirmation required");
        assert!(missing_manual.to_string().contains("--manual"));
        merge(
            &paths,
            false,
            MergeCommand::Done {
                identifier: "TASK-1".to_string(),
                manual: true,
                actor: "tester".to_string(),
            },
        )
        .await
        .expect("mark manually merged done");
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("get task")
            .expect("task");
        assert_eq!(detail.task.state, "done");

        task(
            &paths,
            false,
            TaskCommand::Create {
                bootstrap: true,
                queue: "TASK".to_string(),
                file: temp.path().join("task.md"),
                actor: "tester".to_string(),
            },
        )
        .await
        .expect("create retry task");
        let claimed = tasker_db::claim_next(
            &pool,
            &tasker_db::ClaimNextInput {
                queue_key: "TASK".to_string(),
                worker_id: "worker".to_string(),
                launcher_kind: "fake".to_string(),
                lease_seconds: 90,
            },
            &tasker_db::Actor {
                kind: "worker_agent".to_string(),
                id: "worker".to_string(),
                display_name: "worker".to_string(),
            },
        )
        .await
        .expect("claim retry task")
        .expect("claimed retry task");
        run(
            &paths,
            false,
            RunCommand::Fail {
                run_id: claimed.run.id,
                reason: "operator recovery test".to_string(),
                retry_hold_seconds: Some(60),
                actor: "tester".to_string(),
            },
        )
        .await
        .expect("fail active run");
        task(
            &paths,
            false,
            TaskCommand::Retry {
                identifier: "TASK-2".to_string(),
                reason: "retry after operator failure".to_string(),
                actor: "tester".to_string(),
            },
        )
        .await
        .expect("retry task");
        let retry_detail = tasker_db::get_task_detail(&pool, "TASK-2")
            .await
            .expect("get retry task")
            .expect("retry task");
        assert_eq!(retry_detail.task.state, "ready");
        status(&paths, false).await.expect("status");
    }

    fn init_git_repo(repo: &Path) {
        fs::create_dir_all(repo).expect("repo dir");
        git(repo, &["init", "-b", "main"]);
        git(repo, &["config", "user.email", "tasker@example.test"]);
        git(repo, &["config", "user.name", "Tasker Test"]);
        fs::write(repo.join("README.md"), "test repo\n").expect("readme");
        git(repo, &["add", "README.md"]);
        git(repo, &["commit", "-m", "initial"]);
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn merge_inspection_git_commands_report_cleanliness_and_main_diff() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        init_git_repo(&repo);
        git(&repo, &["checkout", "-b", "tasker/TASK-1"]);
        fs::write(repo.join("feature.txt"), "feature\n").expect("feature");
        git(&repo, &["add", "feature.txt"]);
        git(&repo, &["commit", "-m", "add feature"]);

        assert!(git_output(&repo, &["status", "--porcelain"])
            .expect("status")
            .trim()
            .is_empty());
        assert!(git_output(&repo, &["diff", "--stat", "main...HEAD"])
            .expect("diff stat")
            .contains("feature.txt"));
        assert!(git_output(&repo, &["log", "--oneline", "main..HEAD"])
            .expect("log")
            .contains("add feature"));

        fs::write(repo.join("scratch.txt"), "scratch\n").expect("scratch");
        assert!(git_output(&repo, &["status", "--porcelain"])
            .expect("dirty status")
            .contains("scratch.txt"));
    }

    #[test]
    fn bootstrap_parser_defaults_to_ready_normal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let task_file = temp.path().join("task.md");
        fs::write(
            &task_file,
            "---\ntitle: Test\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Tests pass\n---\nBrief\n",
        )
        .expect("write task file");

        let parsed = bootstrap::parse_bootstrap_task_file("TASK", &task_file).expect("parse");

        assert_eq!(parsed.queue_key, "TASK");
        assert_eq!(parsed.priority, "normal");
        assert_eq!(parsed.state, "ready");
        assert_eq!(parsed.brief, "Brief");
    }

    #[test]
    fn bootstrap_parser_requires_front_matter() {
        let error = bootstrap::parse_bootstrap_task("TASK", "inline", "title: Missing delimiters")
            .expect_err("missing front matter fails");

        assert!(error.to_string().contains("must start"));
    }
}

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
mod cleanup;
mod monitor;
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
    /// Open a read-only terminal Task status monitor.
    #[command(
        after_long_help = "Terminal notes:\n  tasker monitor uses raw mode and the alternate screen for interactive rendering.\n  Use --plain, or --once for a single plain snapshot, when terminal capabilities are limited.\n  Remote terminals and tmux should render normally when TERM is not dumb; if output is piped or TERM=dumb, tasker monitor prints one plain snapshot instead.\n\nSmoke fallback:\n  tasker monitor --queue TASKER --once --plain"
    )]
    Monitor {
        /// Optional Task Queue Key filter.
        #[arg(long)]
        queue: Option<String>,
        /// Refresh interval in seconds.
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..))]
        refresh_seconds: u64,
        /// Print one plain snapshot instead of using terminal control sequences.
        #[arg(long)]
        plain: bool,
        /// Print one snapshot and exit.
        #[arg(long)]
        once: bool,
    },
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
        /// Intentionally bypass the per-Task Queue supervisor lock.
        #[arg(long)]
        allow_overlap: bool,
    },
    /// Inspect Agent Runs.
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
    /// Explicit operator cleanup for local dogfood storage artifacts.
    Cleanup {
        #[command(subcommand)]
        command: CleanupCommand,
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
        /// Main Branch commit that the validation evidence was run against.
        #[arg(long)]
        validated_base_commit: Option<String>,
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
enum CleanupCommand {
    /// Summarize or remove rebuildable Cargo target/ directories under Local Worktrees.
    CargoTargets {
        /// Task Queue Key whose configured Worktree Root should be scanned.
        #[arg(long, conflicts_with = "worktree_root")]
        queue: Option<String>,
        /// Worktree Root to scan directly.
        #[arg(long)]
        worktree_root: Option<PathBuf>,
        /// Explicitly keep files and only report reclaimable space.
        #[arg(long, conflicts_with = "delete")]
        dry_run: bool,
        /// Delete matching rebuildable target/ trees.
        #[arg(long)]
        delete: bool,
    },
    /// Summarize or prune saved Run Transcript and Launcher Session Data artifact files.
    Runs {
        /// Override the Run Transcript root; defaults to <data-dir>/runs.
        #[arg(long)]
        runs_dir: Option<PathBuf>,
        /// Select artifacts older than this many days for pruning.
        #[arg(long)]
        older_than_days: Option<u64>,
        /// Keep the newest N run artifact directories/files and select older ones for pruning.
        #[arg(long)]
        keep_latest: Option<usize>,
        /// Explicitly keep files and only report selected artifact space.
        #[arg(long, conflicts_with = "delete")]
        dry_run: bool,
        /// Delete selected Run Transcript and Launcher Session Data artifacts.
        #[arg(long)]
        delete: bool,
    },
}

#[derive(Debug, Subcommand)]
enum MergeCommand {
    /// List Integrating Tasks for temporary Manual Dogfood Merge inspection.
    Queue {
        /// Optional Task Queue Key filter.
        #[arg(long)]
        queue: Option<String>,
    },
    /// Print a temporary Manual Dogfood Merge inspection plan for a Task.
    Inspect { identifier: String },
    /// Integrate an already-Integrating Task through runner-side Local Worktree Delivery.
    Integrate {
        /// Task Identifier.
        identifier: String,
        /// Operator actor display name for audit attribution.
        #[arg(long, default_value = "local-operator")]
        actor: String,
    },
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
    allow_overlap: bool,
}

#[derive(Debug, Clone, Default)]
struct PathForwardingOptions {
    config: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    db_path: Option<PathBuf>,
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
    print_active_tasker_context_for_mutation(&cli.command, &paths, db_path_overridden)?;

    match cli.command {
        Some(Command::Init) => init(&paths, db_path_overridden).await,
        Some(Command::Queue { command }) => queue(&paths, db_path_overridden, command).await,
        Some(Command::Task { command }) => task(&paths, db_path_overridden, command).await,
        Some(Command::Status) => status(&paths, db_path_overridden).await,
        Some(Command::Monitor {
            queue,
            refresh_seconds,
            plain,
            once,
        }) => {
            monitor(
                &paths,
                db_path_overridden,
                queue,
                refresh_seconds,
                plain,
                once,
            )
            .await
        }
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
            allow_overlap,
        }) => {
            supervise(
                &paths,
                db_path_overridden,
                PathForwardingOptions {
                    config: cli.config.clone(),
                    data_dir: cli.data_dir.clone(),
                    db_path: cli.db_path.clone(),
                },
                SuperviseOptions {
                    queue,
                    concurrency,
                    timeout_seconds,
                    poll_seconds,
                    launcher,
                    worker_command,
                    allow_overlap,
                },
            )
            .await
        }
        Some(Command::Run { command }) => run(&paths, db_path_overridden, command).await,
        Some(Command::Cleanup { command }) => cleanup(&paths, db_path_overridden, command).await,
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

fn cleanup_command_is_unsafe_mutation(command: &CleanupCommand) -> bool {
    match command {
        CleanupCommand::CargoTargets { delete, .. } | CleanupCommand::Runs { delete, .. } => {
            *delete
        }
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
        Some(Command::Cleanup { command }) => cleanup_command_is_unsafe_mutation(command),
        Some(Command::Merge { command }) => {
            matches!(
                command,
                MergeCommand::Integrate { .. } | MergeCommand::Done { .. }
            )
        }
        Some(Command::Status | Command::Monitor { .. } | Command::Version) | None => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ActiveTaskerContext {
    config_path: PathBuf,
    data_dir: PathBuf,
    database_path: PathBuf,
    queue_key: Option<String>,
}

fn active_tasker_context(
    command: &Option<Command>,
    paths: &TaskerPaths,
    db_path_overridden: bool,
) -> Result<ActiveTaskerContext> {
    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }

    Ok(ActiveTaskerContext {
        config_path: paths.config_path.clone(),
        data_dir: paths.data_dir.clone(),
        database_path: config.database.path,
        queue_key: command_queue_key(command),
    })
}

fn command_queue_key(command: &Option<Command>) -> Option<String> {
    match command {
        Some(Command::Queue { command }) => match command {
            QueueCommand::Create { key, .. } | QueueCommand::Update { key, .. } => {
                Some(key.clone())
            }
            QueueCommand::Show { .. } | QueueCommand::Audit { .. } | QueueCommand::List => None,
        },
        Some(Command::Task { command }) => match command {
            TaskCommand::Create { queue, .. } => Some(queue.clone()),
            TaskCommand::Retry { identifier, .. }
            | TaskCommand::Transition { identifier, .. }
            | TaskCommand::Audit { identifier } => queue_key_from_task_identifier(identifier),
            TaskCommand::Criterion { command } | TaskCommand::Validation { command } => {
                requirement_command_queue_key(command)
            }
            TaskCommand::Workpad { command } => workpad_command_queue_key(command),
            TaskCommand::Show { .. } => None,
        },
        Some(
            Command::Work { queue, .. }
            | Command::Supervise { queue, .. }
            | Command::Monitor {
                queue: Some(queue), ..
            },
        ) => Some(queue.clone()),
        Some(Command::Monitor { queue: None, .. } | Command::Cleanup { .. }) => None,
        Some(Command::Merge { command }) => match command {
            MergeCommand::Queue { queue } => queue.clone(),
            MergeCommand::Inspect { .. } => None,
            MergeCommand::Integrate { identifier, .. } | MergeCommand::Done { identifier, .. } => {
                queue_key_from_task_identifier(identifier)
            }
        },
        Some(Command::Init | Command::Run { .. } | Command::Serve { .. })
        | Some(Command::Status | Command::Version)
        | None => None,
    }
}

fn requirement_command_queue_key(command: &RequirementCommand) -> Option<String> {
    match command {
        RequirementCommand::Set { identifier, .. } => queue_key_from_task_identifier(identifier),
    }
}

fn workpad_command_queue_key(command: &WorkpadCommand) -> Option<String> {
    match command {
        WorkpadCommand::Set { identifier, .. } => queue_key_from_task_identifier(identifier),
    }
}

fn queue_key_from_task_identifier(identifier: &str) -> Option<String> {
    identifier
        .split_once('-')
        .and_then(|(queue_key, _)| (!queue_key.is_empty()).then(|| queue_key.to_string()))
}

fn render_active_tasker_context(context: &ActiveTaskerContext) -> String {
    let mut output = format!(
        "active Tasker context:\n  config: {}\n  data: {}\n  database: {}",
        context.config_path.display(),
        context.data_dir.display(),
        context.database_path.display()
    );
    if let Some(queue_key) = &context.queue_key {
        output.push_str(&format!("\n  Task Queue Key: {queue_key}"));
    }
    output
}

fn print_active_tasker_context_for_mutation(
    command: &Option<Command>,
    paths: &TaskerPaths,
    db_path_overridden: bool,
) -> Result<()> {
    if !command_is_unsafe_mutation(command) {
        return Ok(());
    }

    let context = active_tasker_context(command, paths, db_path_overridden)?;
    eprintln!("{}", render_active_tasker_context(&context));
    Ok(())
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
                validated_base_commit: _,
                actor,
            } => {
                let input = tasker_db::UpdateRequirementStatus {
                    status: bootstrap::normalize_label(&status),
                    waiver_reason,
                    validated_base_commit: None,
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
                validated_base_commit,
                actor,
            } => {
                let status = bootstrap::normalize_label(&status);
                let validated_base_commit = validation_base_commit_for_status(
                    &pool,
                    &identifier,
                    &status,
                    validated_base_commit,
                )
                .await?;
                let input = tasker_db::UpdateRequirementStatus {
                    status,
                    waiver_reason,
                    validated_base_commit,
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
    let conflict_groups = tasker_db::task_conflict_groups_for_status(&pool).await?;

    let mut current_queue: Option<String> = None;
    for row in rows {
        let queue_header = format!("{}\t{}", row.queue_key, row.queue_name);
        if current_queue.as_ref() != Some(&queue_header) {
            if current_queue.is_some() {
                println!();
            }
            println!("Task Queue: {queue_header}");
            match row.queue_concurrency_limit {
                Some(limit) => {
                    let available = (limit - row.active_agent_runs).max(0);
                    println!(
                        "  active Agent Runs: {} / Queue Concurrency Limit {limit}",
                        row.active_agent_runs
                    );
                    println!("  available claim slots: {available}");
                    if available == 0 && row.ready_tasks > 0 {
                        println!(
                            "  claim status: limit reached; Ready Tasks cannot be claimed until active Agent Runs finish"
                        );
                    }
                }
                None => {
                    println!(
                        "  active Agent Runs: {} / Queue Concurrency Limit none",
                        row.active_agent_runs
                    );
                    println!("  available claim slots: unlimited");
                }
            }
            println!("  Ready Tasks: {}", row.ready_tasks);
            println!("  Integrating Tasks: {}", row.integrating_tasks);
            println!(
                "  active Agent Runs on Integrating Tasks: {}",
                row.active_integrating_agent_runs
            );
            let queue_active_runs: Vec<_> = active_runs
                .iter()
                .filter(|run| run.queue_key.as_str() == row.queue_key.as_str())
                .collect();
            for run in &queue_active_runs {
                println!(
                    "    {}\tstate={}\t{}\tlauncher={}\tworker={}\tlease_expires_at={}",
                    run.task_identifier,
                    run.task_state,
                    run.agent_run_id,
                    run.launcher_kind,
                    run.worker_id,
                    run.lease_expires_at
                );
            }
            if let Some(limit) = row.queue_concurrency_limit {
                if row.active_agent_runs >= limit {
                    let active_integrating = queue_active_runs
                        .iter()
                        .filter(|run| run.task_state == "integrating")
                        .count();
                    if active_integrating > 0 {
                        println!(
                            "  capacity saturated: active Agent Runs count against Queue Concurrency Limit, including {active_integrating} Integrating run(s). Unblock by waiting for completion or lease expiry, inspecting/finishing stuck runs with `tasker run show`/`tasker run fail`, or raising/clearing the Queue Concurrency Limit only if local resources permit."
                        );
                    } else {
                        println!(
                            "  capacity saturated: active Agent Runs count against Queue Concurrency Limit. Unblock by waiting for completion or lease expiry, inspecting/finishing stuck runs with `tasker run show`/`tasker run fail`, or raising/clearing the Queue Concurrency Limit only if local resources permit."
                        );
                    }
                }
            }
            let queue_conflict_groups: Vec<_> = conflict_groups
                .iter()
                .filter(|group| group.queue_key.as_str() == row.queue_key.as_str())
                .collect();
            if !queue_conflict_groups.is_empty() {
                println!("  advisory Task conflict hints:");
                for group in queue_conflict_groups {
                    println!(
                        "    {}\t{} Task(s): {}",
                        group.target, group.task_count, group.tasks
                    );
                }
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

async fn monitor(
    paths: &TaskerPaths,
    db_path_overridden: bool,
    queue: Option<String>,
    refresh_seconds: u64,
    plain: bool,
    once: bool,
) -> Result<()> {
    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    let pool = tasker_db::connect(&config.database.path).await?;
    tasker_db::run_migrations(&pool).await?;
    monitor::run_monitor(
        &pool,
        monitor::MonitorOptions {
            queue,
            refresh_seconds,
            plain,
            once,
            config_path: paths.config_path.clone(),
            db_path: config.database.path,
        },
    )
    .await
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
    path_forwarding: PathForwardingOptions,
    options: SuperviseOptions,
) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;
    let database_path = resolved_database_path(paths, db_path_overridden)?;
    let command = if let Some(command) = options.worker_command {
        command
    } else {
        let exe = std::env::current_exe().context("failed to resolve current tasker executable")?;
        default_worker_command(&exe, &path_forwarding, &options)
    };

    println!(
        "supervisor worker context: config={} data_dir={} database={}",
        paths.config_path.display(),
        paths.data_dir.display(),
        database_path.display()
    );

    let outcome = supervisor::supervise_batch(
        &pool,
        supervisor::SupervisorOptions {
            queue: options.queue,
            concurrency: options.concurrency,
            timeout_seconds: options.timeout_seconds,
            poll_seconds: options.poll_seconds,
            worker_command: command,
            lock_dir: paths.data_dir.join("supervisors"),
            allow_overlap: options.allow_overlap,
            #[cfg(test)]
            run_prefix: None,
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

fn default_worker_command(
    exe: &Path,
    path_forwarding: &PathForwardingOptions,
    options: &SuperviseOptions,
) -> Vec<String> {
    let mut command = vec![exe.display().to_string()];
    if let Some(config) = &path_forwarding.config {
        command.push("--config".to_string());
        command.push(config.display().to_string());
    }
    if let Some(data_dir) = &path_forwarding.data_dir {
        command.push("--data-dir".to_string());
        command.push(data_dir.display().to_string());
    }
    if let Some(db_path) = &path_forwarding.db_path {
        command.push("--db-path".to_string());
        command.push(db_path.display().to_string());
    }
    command.extend([
        "work".to_string(),
        "--once".to_string(),
        "--queue".to_string(),
        options.queue.clone(),
        "--launcher".to_string(),
        options.launcher.clone(),
    ]);
    command
}

fn resolved_database_path(paths: &TaskerPaths, db_path_overridden: bool) -> Result<PathBuf> {
    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    Ok(config.database.path)
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

async fn cleanup(
    paths: &TaskerPaths,
    db_path_overridden: bool,
    command: CleanupCommand,
) -> Result<()> {
    match command {
        CleanupCommand::CargoTargets {
            queue,
            worktree_root,
            dry_run: _,
            delete,
        } => {
            let worktree_root = if let Some(root) = worktree_root {
                root
            } else if let Some(queue_key) = queue {
                let pool = open_pool(paths, db_path_overridden).await?;
                let queue = tasker_db::get_task_queue(&pool, &queue_key)
                    .await?
                    .with_context(|| format!("Task Queue {queue_key} not found"))?;
                PathBuf::from(queue.worktree_root)
            } else {
                anyhow::bail!("cleanup cargo-targets requires --queue or --worktree-root");
            };
            let report = cleanup::cleanup_cargo_targets(&worktree_root, delete)?;
            println!("Local Worktree Cargo target cleanup");
            println!("Worktree Root: {}", worktree_root.display());
            println!("mode: {}", if delete { "delete" } else { "dry-run" });
            println!(
                "safe-to-delete artifact kind: rebuildable per-Local Worktree target/ directories"
            );
            println!("preserved Task data: Local Worktree source files, Task Branches, Task records, Agent Runs, and Audit Events");
            print_cleanup_report(&report);
        }
        CleanupCommand::Runs {
            runs_dir,
            older_than_days,
            keep_latest,
            dry_run: _,
            delete,
        } => {
            let runs_dir = runs_dir.unwrap_or_else(|| paths.data_dir.join("runs"));
            let report = cleanup::cleanup_run_artifacts(
                &runs_dir,
                cleanup::RunPruneOptions {
                    older_than_days,
                    keep_latest,
                },
                delete,
            )?;
            println!("Run Transcript and Launcher Session Data artifact cleanup");
            println!("Run artifact root: {}", runs_dir.display());
            println!("mode: {}", if delete { "delete" } else { "dry-run" });
            if let Some(days) = older_than_days {
                println!("selection: older than {days} day(s)");
            }
            if let Some(keep) = keep_latest {
                println!("selection: keep newest {keep} artifact(s)");
            }
            if older_than_days.is_none() && keep_latest.is_none() {
                println!("selection: summarize all artifacts");
            }
            println!("safe-to-delete artifact kind: saved Run Transcript files and launcher raw/session artifacts under runs/");
            println!("preserved authoritative data: Task records, Agent Run rows, Launcher Session Data database rows, and Audit Events");
            print_cleanup_report(&report);
        }
    }
    Ok(())
}

fn print_cleanup_report(report: &cleanup::CleanupReport) {
    println!(
        "{} entries, {} reclaimable",
        report.entries.len(),
        cleanup::human_bytes(report.total_bytes())
    );
    for entry in &report.entries {
        println!(
            "  {}	{}",
            cleanup::human_bytes(entry.bytes),
            entry.path.display()
        );
    }
}

async fn merge(paths: &TaskerPaths, db_path_overridden: bool, command: MergeCommand) -> Result<()> {
    let pool = open_pool(paths, db_path_overridden).await?;
    match command {
        MergeCommand::Queue { queue } => {
            let rows = tasker_db::merge_queue_tasks(&pool, queue.as_deref()).await?;
            print_manual_merge_queue(&rows);
        }
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
        MergeCommand::Integrate { identifier, actor } => {
            let actor = tasker_db::Actor::operator(actor);
            let outcome = integrate_local_worktree(&pool, &identifier, &actor).await?;
            println!("{}", outcome.summary);
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalIntegrationResult {
    summary: String,
}

async fn integrate_local_worktree(
    pool: &sqlx::SqlitePool,
    identifier: &str,
    actor: &tasker_db::Actor,
) -> Result<LocalIntegrationResult> {
    let detail = tasker_db::get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("Task {identifier} not found"))?;
    if detail.task.state != "integrating" {
        anyhow::bail!(
            "Local Worktree integration requires Task State integrating; current state is {}",
            detail.task.state
        );
    }
    let queue = tasker_db::get_task_queue(pool, &detail.task.task_queue_key)
        .await?
        .with_context(|| format!("Task Queue {} not found", detail.task.task_queue_key))?;
    let task_branch = required_task_link(&detail, "task_branch")?;
    let local_worktree = required_task_link(&detail, "local_worktree")?;
    let latest_run = tasker_db::get_latest_agent_run_detail_for_task(pool, identifier).await?;
    let agent_run_id = latest_run.map(|run| run.run.id);

    let repo = Path::new(&queue.managed_source_repository);
    let worktree = Path::new(&local_worktree);
    let mut adapter = LocalWorktreeIntegrationAdapter {
        pool,
        task: &detail,
        queue: &queue,
        actor,
        agent_run_id: agent_run_id.as_deref(),
        repo,
        worktree,
        task_branch: &task_branch,
    };
    adapter.integrate().await
}

async fn validation_base_commit_for_status(
    pool: &sqlx::SqlitePool,
    identifier: &str,
    status: &str,
    provided: Option<String>,
) -> Result<Option<String>> {
    if status != "passed" {
        return Ok(None);
    }
    if let Some(commit) = provided {
        let commit = commit.trim().to_string();
        if !commit.is_empty() {
            return Ok(Some(commit));
        }
    }

    let detail = tasker_db::get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("Task {identifier} not found"))?;
    let queue = tasker_db::get_task_queue(pool, &detail.task.task_queue_key)
        .await?
        .with_context(|| format!("Task Queue {} not found", detail.task.task_queue_key))?;
    let commit = git_output(
        Path::new(&queue.managed_source_repository),
        &["rev-parse", &queue.main_branch],
    )?
    .trim()
    .to_string();
    Ok(Some(commit))
}

fn required_task_link(detail: &tasker_db::TaskDetail, kind: &str) -> Result<String> {
    detail
        .task_links
        .iter()
        .find(|link| link.kind == kind)
        .map(|link| link.target.clone())
        .with_context(|| {
            format!(
                "Task {} is missing {kind} Task Link",
                detail.task.identifier
            )
        })
}

struct LocalWorktreeIntegrationAdapter<'a> {
    pool: &'a sqlx::SqlitePool,
    task: &'a tasker_db::TaskDetail,
    queue: &'a tasker_db::TaskQueue,
    actor: &'a tasker_db::Actor,
    agent_run_id: Option<&'a str>,
    repo: &'a Path,
    worktree: &'a Path,
    task_branch: &'a str,
}

impl<'a> LocalWorktreeIntegrationAdapter<'a> {
    async fn integrate(&mut self) -> Result<LocalIntegrationResult> {
        if let Err(error) = self.validate_operational_safety() {
            self.record_outcome("operational_failure", None, None, Some(error.to_string()))
                .await?;
            return Ok(LocalIntegrationResult {
                summary: format!(
                    "operational Delivery Failure for Task {}; left in Integrating: {error:#}",
                    self.task.task.identifier
                ),
            });
        }
        if let Err(error) = self.validate_work_change_safety() {
            self.record_outcome("work_change_failure", None, None, Some(error.to_string()))
                .await?;
            self.transition("rework").await?;
            return Ok(LocalIntegrationResult {
                summary: format!(
                    "work-change Delivery Failure for Task {}; moved to Rework: {error:#}",
                    self.task.task.identifier
                ),
            });
        }

        let pre_merge_head = git_output(self.repo, &["rev-parse", &self.queue.main_branch])?
            .trim()
            .to_string();
        if git_status(
            self.repo,
            &[
                "diff",
                "--quiet",
                &format!("{}..{}", self.queue.main_branch, self.task_branch),
                "--",
            ],
        )?
        .success()
        {
            self.record_outcome("no_changes", None, Some(pre_merge_head.clone()), None)
                .await?;
            self.transition("done").await?;
            let cleanup = self.cleanup_after_success();
            let mut summary = format!(
                "No-Change Integration recorded for Task {}; moved to Done",
                self.task.task.identifier
            );
            if let Err(error) = cleanup {
                summary.push_str(&format!("; cleanup needs operator repair: {error:#}"));
            }
            return Ok(LocalIntegrationResult { summary });
        }

        if let Err(error) = run_git(
            self.repo,
            &["merge", "--squash", "--no-commit", self.task_branch],
        ) {
            let _ = self.rollback_to(&pre_merge_head);
            self.record_outcome(
                "work_change_failure",
                None,
                Some(pre_merge_head.clone()),
                Some(format!("Squash Merge failed: {error:#}")),
            )
            .await?;
            self.transition("rework").await?;
            return Ok(LocalIntegrationResult {
                summary: format!(
                    "work-change Delivery Failure for Task {}; moved to Rework: Squash Merge failed: {error:#}",
                    self.task.task.identifier
                ),
            });
        }

        let message = final_commit_message(self.task, self.agent_run_id);
        if let Err(error) = run_git(self.repo, &["commit", "-m", &message]) {
            let _ = self.rollback_to(&pre_merge_head);
            self.record_outcome(
                "work_change_failure",
                None,
                Some(pre_merge_head.clone()),
                Some(format!("Final Commit failed: {error:#}")),
            )
            .await?;
            self.transition("rework").await?;
            return Ok(LocalIntegrationResult {
                summary: format!(
                    "work-change Delivery Failure for Task {}; moved to Rework: Final Commit failed: {error:#}",
                    self.task.task.identifier
                ),
            });
        }

        let final_commit = git_output(self.repo, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        self.record_outcome(
            "success",
            Some(final_commit.clone()),
            Some(pre_merge_head),
            None,
        )
        .await
        .with_context(|| {
            format!(
                "Final Commit {final_commit} was created but Tasker could not record the successful Integration Outcome; operator repair required"
            )
        })?;
        self.transition("done").await.with_context(|| {
            format!(
                "Final Commit {final_commit} was created but Tasker could not move the Task to Done; operator repair required"
            )
        })?;
        let cleanup = self.cleanup_after_success();
        let mut summary = format!(
            "Integrated Task {} as Final Commit {}; moved to Done",
            self.task.task.identifier, final_commit
        );
        if let Err(error) = cleanup {
            summary.push_str(&format!("; cleanup needs operator repair: {error:#}"));
        }
        Ok(LocalIntegrationResult { summary })
    }

    fn validate_operational_safety(&self) -> Result<()> {
        run_git(self.repo, &["rev-parse", "--show-toplevel"])?;
        run_git(self.worktree, &["rev-parse", "--is-inside-work-tree"])?;
        ensure_no_git_lock(self.repo)?;
        ensure_no_git_lock(self.worktree)?;
        let branch = git_output(self.repo, &["branch", "--show-current"])?;
        if branch.trim() != self.queue.main_branch {
            anyhow::bail!(
                "Managed Source Repository is on branch {}, expected Main Branch {}",
                branch.trim(),
                self.queue.main_branch
            );
        }
        ensure_clean_git(self.repo, "Managed Source Repository")?;
        let source_common_dir = git_common_dir_for_cli(self.repo)?;
        let worktree_common_dir = git_common_dir_for_cli(self.worktree)?;
        if source_common_dir != worktree_common_dir {
            anyhow::bail!(
                "Local Worktree is not attached to the configured Managed Source Repository"
            );
        }
        if !git_status(
            self.repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{}", self.task_branch),
            ],
        )?
        .success()
        {
            anyhow::bail!("Task Branch {} does not exist", self.task_branch);
        }
        Ok(())
    }

    fn validate_work_change_safety(&self) -> Result<()> {
        ensure_clean_git(self.worktree, "Local Worktree")?;
        let worktree_branch = git_output(self.worktree, &["branch", "--show-current"])?;
        if worktree_branch.trim() != self.task_branch {
            anyhow::bail!(
                "Local Worktree is on branch {}, expected Task Branch {}",
                worktree_branch.trim(),
                self.task_branch
            );
        }
        let current_main = git_output(self.repo, &["rev-parse", &self.queue.main_branch])?
            .trim()
            .to_string();
        if !git_status(
            self.repo,
            &[
                "merge-base",
                "--is-ancestor",
                &self.queue.main_branch,
                self.task_branch,
            ],
        )?
        .success()
            && self.task.task.validated_base_commit.as_deref() != Some(current_main.as_str())
        {
            anyhow::bail!(
                "Task Branch {} does not include current Main Branch {} and Validated Base Commit {} is stale or missing (current Main Branch is {})",
                self.task_branch,
                self.queue.main_branch,
                self.task
                    .task
                    .validated_base_commit
                    .as_deref()
                    .unwrap_or("not recorded"),
                current_main
            );
        }
        Ok(())
    }

    async fn record_outcome(
        &self,
        kind: &str,
        final_commit: Option<String>,
        pre_merge_head: Option<String>,
        message: Option<String>,
    ) -> Result<()> {
        tasker_db::record_integration_outcome(
            self.pool,
            &tasker_db::RecordIntegrationOutcomeInput {
                task_identifier: self.task.task.identifier.clone(),
                agent_run_id: self.agent_run_id.map(ToString::to_string),
                outcome_kind: kind.to_string(),
                final_commit,
                pre_merge_head,
                message,
            },
            self.actor,
        )
        .await?;
        Ok(())
    }

    async fn transition(&self, to_state: &str) -> Result<()> {
        tasker_db::transition_task_state(
            self.pool,
            &self.task.task.identifier,
            &tasker_db::TransitionTaskState {
                to_state: to_state.to_string(),
                agent_run_id: None,
            },
            self.actor,
        )
        .await?;
        Ok(())
    }

    fn rollback_to(&self, pre_merge_head: &str) -> Result<()> {
        let branch = git_output(self.repo, &["branch", "--show-current"])?;
        if branch.trim() != self.queue.main_branch {
            anyhow::bail!(
                "refusing rollback because Managed Source Repository is no longer on Main Branch"
            );
        }
        run_git(self.repo, &["reset", "--hard", pre_merge_head])
    }

    fn cleanup_after_success(&self) -> Result<()> {
        if self.queue.done_worktree_retention {
            return Ok(());
        }
        if self.worktree.exists() {
            let worktree = self.worktree.display().to_string();
            run_git(self.repo, &["worktree", "remove", "--force", &worktree])?;
        }
        if git_status(
            self.repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{}", self.task_branch),
            ],
        )?
        .success()
        {
            run_git(self.repo, &["branch", "-D", self.task_branch])?;
        }
        Ok(())
    }
}

fn final_commit_message(detail: &tasker_db::TaskDetail, agent_run_id: Option<&str>) -> String {
    let mut message = format!(
        "{}: {}\n\nTask: {}",
        detail.task.identifier, detail.task.title, detail.task.identifier
    );
    if let Some(run_id) = agent_run_id {
        message.push_str(&format!("\nAgent Run: {run_id}"));
    }
    message
}

fn ensure_clean_git(repo: &Path, label: &str) -> Result<()> {
    let status = git_output(repo, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        anyhow::bail!("{label} has uncommitted changes");
    }
    Ok(())
}

fn ensure_no_git_lock(repo: &Path) -> Result<()> {
    let common_dir = git_common_dir_for_cli(repo)?;
    if common_dir.join("index.lock").exists() {
        anyhow::bail!(
            "Git lock exists at {}",
            common_dir.join("index.lock").display()
        );
    }
    let git_dir = git_output(repo, &["rev-parse", "--git-dir"])?;
    let git_dir = PathBuf::from(git_dir.trim());
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        repo.join(git_dir)
    };
    if git_dir.join("index.lock").exists() {
        anyhow::bail!(
            "Git lock exists at {}",
            git_dir.join("index.lock").display()
        );
    }
    Ok(())
}

fn git_common_dir_for_cli(repo: &Path) -> Result<PathBuf> {
    let common_dir = git_output(repo, &["rev-parse", "--git-common-dir"])?;
    let path = PathBuf::from(common_dir.trim());
    let absolute = if path.is_absolute() {
        path
    } else {
        repo.join(path)
    };
    absolute
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", absolute.display()))
}

fn run_git(repo: &Path, args: &[&str]) -> Result<()> {
    git_output(repo, args).map(|_| ())
}

fn git_status(repo: &Path, args: &[&str]) -> Result<std::process::ExitStatus> {
    ProcessCommand::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .with_context(|| format!("failed to run git {:?} in {}", args, repo.display()))
}

fn print_manual_merge_queue(rows: &[tasker_db::MergeQueueTask]) {
    println!("Manual Dogfood Merge queue");
    println!("temporary helper: read-only view for Integrating Tasks; Git operations remain operator-side; Tasker Service performs no Git mutations");
    println!("Tasks: {}", rows.len());
    if rows.is_empty() {
        println!("(none)");
        return;
    }
    println!();
    for row in rows {
        let git = inspect_merge_queue_git(
            row.local_worktree.as_deref(),
            row.task_branch.as_deref(),
            &row.main_branch,
        );
        let gates_ready = row.pending_acceptance_criteria == 0
            && row.pending_validation_items == 0
            && row.failed_validation_items == 0;
        let has_task_commit = git.task_commits.unwrap_or(false);
        let clean = git.clean.unwrap_or(false);
        let ready = gates_ready && clean && has_task_commit;
        println!(
            "{} [{}] {}",
            row.task_identifier,
            if ready { "ready" } else { "attention" },
            row.title
        );
        println!("  Task Queue: {}", row.queue_key);
        println!(
            "  Task Branch: {}",
            row.task_branch.as_deref().unwrap_or("missing Task Link")
        );
        println!(
            "  Local Worktree: {}",
            row.local_worktree.as_deref().unwrap_or("missing Task Link")
        );
        println!("  Main Branch: {}", row.main_branch);
        println!(
            "  Latest Agent Run: {} ({})",
            row.latest_agent_run_id.as_deref().unwrap_or("none"),
            row.latest_agent_run_outcome.as_deref().unwrap_or("none")
        );
        println!(
            "  Structured gates: {} pending Acceptance Criteria, {} pending Validation Items, {} failed Validation Items",
            row.pending_acceptance_criteria, row.pending_validation_items, row.failed_validation_items
        );
        println!("  Local Worktree clean: {}", git.label(git.clean));
        println!("  Task Commits present: {}", git.label(git.task_commits));
        println!(
            "  Merge inspection readiness: {}",
            if ready {
                "clean and gate-satisfied"
            } else {
                "operator attention needed"
            }
        );
        if let Some(warning) = git.warning {
            println!("  Attention: {warning}");
        }
        println!("  Detail: tasker merge inspect {}", row.task_identifier);
        println!();
    }
}

#[derive(Debug, Default)]
struct MergeQueueGitInspection {
    clean: Option<bool>,
    task_commits: Option<bool>,
    warning: Option<String>,
}

impl MergeQueueGitInspection {
    fn label(&self, value: Option<bool>) -> &'static str {
        match value {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        }
    }
}

fn inspect_merge_queue_git(
    local_worktree: Option<&str>,
    task_branch: Option<&str>,
    main_branch: &str,
) -> MergeQueueGitInspection {
    let Some(local_worktree) = local_worktree else {
        return MergeQueueGitInspection {
            warning: Some("missing Local Worktree Task Link".to_string()),
            ..MergeQueueGitInspection::default()
        };
    };
    let worktree = Path::new(local_worktree);
    if !worktree.exists() {
        return MergeQueueGitInspection {
            warning: Some("Local Worktree path does not exist".to_string()),
            ..MergeQueueGitInspection::default()
        };
    }

    let clean = git_output(worktree, &["status", "--porcelain"])
        .ok()
        .map(|status| status.trim().is_empty());
    let checked_out_branch = git_output(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .map(|branch| branch.trim().to_string());
    let warning = match (checked_out_branch.as_deref(), task_branch) {
        (Some(actual), Some(expected)) if actual != expected => Some(format!(
            "checked-out branch {actual} differs from Task Branch {expected}"
        )),
        _ => None,
    };
    let commits = format!("{main_branch}..HEAD");
    let task_commits = git_output(worktree, &["log", "--oneline", &commits])
        .ok()
        .map(|log| !log.trim().is_empty());

    MergeQueueGitInspection {
        clean,
        task_commits,
        warning,
    }
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
        "Validated Base Commit: {}",
        detail
            .task
            .validated_base_commit
            .as_deref()
            .unwrap_or("not recorded")
    );
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
    println!("Post-merge batch validation:");
    for line in post_merge_batch_validation_guidance() {
        println!("  {line}");
    }
    println!();
    println!("Operator-side checklist:");
    println!("  1. Inspect Tasker state, latest Agent Run, Run Transcript, and Workpad Note.");
    println!("  2. From the Local Worktree, verify a clean working tree and focused Task Commits.");
    println!("  3. Run current validation from the Local Worktree after any refresh.");
    println!("  4. Perform the Local Merge into the Main Branch outside Tasker.");
    println!("  5. From the Managed Source Repository, run post-merge batch validation before marking the batch Done.");
    println!(
        "  6. After validation, run: tasker merge done {} --manual",
        detail.task.identifier
    );
}

fn post_merge_batch_validation_guidance() -> &'static [&'static str] {
    &[
        "After each Local Merge in a Manual Dogfood Merge batch, validate the combined Main Branch; do not rely only on per-Task Local Worktree validation.",
        "Run at least: cargo test",
        "Run at least: cargo clippy --all-targets --all-features -- -D warnings",
        "This catches overlapping CLI/API changes where individual Task Branches passed but the combined Main Branch can fail to compile.",
        "Temporary Manual Dogfood Merge guidance only; it does not replace the target Integrating implementation or automated Squash Merge.",
    ]
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
    fn monitor_help_documents_plain_tmux_and_remote_terminal_expectations() {
        let mut command = Cli::command();
        let monitor = command
            .find_subcommand_mut("monitor")
            .expect("monitor subcommand");
        let help = monitor.render_long_help().to_string();

        assert!(help.contains("raw mode"));
        assert!(help.contains("Remote terminals and tmux should render normally"));
        assert!(help.contains("TERM=dumb"));
        assert!(help.contains("tasker monitor --queue TASKER --once --plain"));
    }

    #[test]
    fn merge_inspect_guidance_includes_post_merge_batch_validation() {
        let guidance = post_merge_batch_validation_guidance().join("\n");

        assert!(guidance.contains("combined Main Branch"));
        assert!(guidance.contains("cargo test"));
        assert!(guidance.contains("cargo clippy --all-targets --all-features -- -D warnings"));
        assert!(guidance.contains("overlapping CLI/API changes"));
        assert!(guidance.contains("does not replace the target Integrating implementation"));
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
    fn supervise_default_worker_command_forwards_only_explicit_config_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("repo/.tasker/config.toml");
        let configured_db = temp.path().join("repo/.tasker/data/project.db");
        write_config(&config_path, &configured_db);
        let paths = TaskerPaths::resolve(
            temp.path().join("home"),
            PathOverrides {
                config_path: Some(config_path.clone()),
                ..PathOverrides::default()
            },
        );

        let command = default_worker_command(
            Path::new("/tmp/tasker"),
            &PathForwardingOptions {
                config: Some(config_path.clone()),
                data_dir: None,
                db_path: None,
            },
            &SuperviseOptions {
                queue: "TASK".to_string(),
                concurrency: 1,
                timeout_seconds: 60,
                poll_seconds: 5,
                launcher: "pi".to_string(),
                worker_command: None,
                allow_overlap: false,
            },
        );

        assert_eq!(
            command,
            vec![
                "/tmp/tasker".to_string(),
                "--config".to_string(),
                config_path.display().to_string(),
                "work".to_string(),
                "--once".to_string(),
                "--queue".to_string(),
                "TASK".to_string(),
                "--launcher".to_string(),
                "pi".to_string(),
            ]
        );
        assert!(!command.contains(&"--data-dir".to_string()));
        assert!(!command.contains(&"--db-path".to_string()));
        assert_eq!(
            resolved_database_path(&paths, false).expect("database path"),
            configured_db
        );
    }

    #[test]
    fn supervise_default_worker_command_forwards_explicit_db_path() {
        let command = default_worker_command(
            Path::new("/tmp/tasker"),
            &PathForwardingOptions {
                config: Some(PathBuf::from(".tasker/config.toml")),
                data_dir: Some(PathBuf::from(".tasker/data")),
                db_path: Some(PathBuf::from(".tasker/data/tasker.db")),
            },
            &SuperviseOptions {
                queue: "TASK".to_string(),
                concurrency: 1,
                timeout_seconds: 60,
                poll_seconds: 5,
                launcher: "fake".to_string(),
                worker_command: None,
                allow_overlap: false,
            },
        );

        assert!(command
            .windows(2)
            .any(|args| args == ["--config", ".tasker/config.toml"]));
        assert!(command
            .windows(2)
            .any(|args| args == ["--data-dir", ".tasker/data"]));
        assert!(command
            .windows(2)
            .any(|args| args == ["--db-path", ".tasker/data/tasker.db"]));
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

    #[test]
    fn active_context_for_task_creation_renders_paths_and_queue_key() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = TaskerPaths::resolve(temp.path(), PathOverrides::default());
        let configured_db = temp.path().join("project/tasker.db");
        write_config(&paths.config_path, &configured_db);
        let command = Some(Command::Task {
            command: TaskCommand::Create {
                bootstrap: true,
                queue: "TASK".to_string(),
                file: temp.path().join("task.md"),
                actor: "tester".to_string(),
            },
        });

        let context = active_tasker_context(&command, &paths, false).expect("active context");
        let rendered = render_active_tasker_context(&context);

        assert!(rendered.contains(&format!("config: {}", paths.config_path.display())));
        assert!(rendered.contains(&format!("data: {}", paths.data_dir.display())));
        assert!(rendered.contains(&format!("database: {}", configured_db.display())));
        assert!(rendered.contains("Task Queue Key: TASK"));
    }

    #[test]
    fn active_context_for_supervisor_renders_queue_key_and_db_override() {
        let temp = tempfile::tempdir().expect("tempdir");
        let override_db = temp.path().join("override/tasker.db");
        let paths = TaskerPaths::resolve(
            temp.path(),
            PathOverrides {
                db_path: Some(override_db.clone()),
                ..PathOverrides::default()
            },
        );
        let command = Some(Command::Supervise {
            queue: "DOG".to_string(),
            concurrency: 2,
            timeout_seconds: 30,
            poll_seconds: 1,
            launcher: "fake".to_string(),
            worker_command: None,
            allow_overlap: false,
        });

        let context = active_tasker_context(&command, &paths, true).expect("active context");
        let rendered = render_active_tasker_context(&context);

        assert!(rendered.contains(&format!("database: {}", override_db.display())));
        assert!(rendered.contains("Task Queue Key: DOG"));
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
                    validated_base_commit: None,
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
                    validated_base_commit: None,
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
        let merge_queue = tasker_db::merge_queue_tasks(&pool, Some("TASK"))
            .await
            .expect("merge queue snapshot");
        assert_eq!(merge_queue.len(), 1);
        assert_eq!(merge_queue[0].task_identifier, "TASK-1");
        assert_eq!(
            merge_queue[0].latest_agent_run_outcome.as_deref(),
            Some("completed")
        );
        assert_eq!(merge_queue[0].pending_acceptance_criteria, 0);
        assert_eq!(merge_queue[0].pending_validation_items, 0);
        merge(
            &paths,
            false,
            MergeCommand::Queue {
                queue: Some("TASK".to_string()),
            },
        )
        .await
        .expect("list manual merge queue");
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

    #[tokio::test]
    async fn merge_integrate_squash_merges_and_cleans_successful_task() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;

        let result =
            integrate_local_worktree(&pool, "TASK-1", &tasker_db::Actor::operator("tester"))
                .await
                .expect("integrate");

        assert!(result.summary.contains("Final Commit"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "done");
        assert!(git_output(&repo, &["show", "--stat", "--oneline", "HEAD"])
            .expect("show final commit")
            .contains("feature.txt"));
        assert!(!worktree.exists());
        assert!(!std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/tasker/TASK-1"
            ])
            .status()
            .expect("branch status")
            .success());
        let outcomes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM integration_outcomes WHERE outcome_kind = 'success'",
        )
        .fetch_one(&pool)
        .await
        .expect("outcome count");
        assert_eq!(outcomes, 1);
    }

    #[tokio::test]
    async fn merge_integrate_records_no_change_without_commit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), false, false).await;
        let before = git_output(&repo, &["rev-parse", "HEAD"]).expect("head");

        let result =
            integrate_local_worktree(&pool, "TASK-1", &tasker_db::Actor::operator("tester"))
                .await
                .expect("integrate");

        assert!(result.summary.contains("No-Change Integration"));
        let after = git_output(&repo, &["rev-parse", "HEAD"]).expect("head");
        assert_eq!(before, after);
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "done");
        let outcomes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM integration_outcomes WHERE outcome_kind = 'no_changes'",
        )
        .fetch_one(&pool)
        .await
        .expect("outcome count");
        assert_eq!(outcomes, 1);
    }

    #[tokio::test]
    async fn merge_integrate_dirty_managed_source_repository_stays_integrating() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        fs::write(repo.join("operator-scratch.txt"), "dirty\n").expect("dirty repo");

        let result =
            integrate_local_worktree(&pool, "TASK-1", &tasker_db::Actor::operator("tester"))
                .await
                .expect("integrate");

        assert!(result.summary.contains("operational Delivery Failure"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "integrating");
        let outcomes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM integration_outcomes WHERE outcome_kind = 'operational_failure'",
        )
        .fetch_one(&pool)
        .await
        .expect("outcome count");
        assert_eq!(outcomes, 1);
    }

    #[tokio::test]
    async fn merge_integrate_dirty_local_worktree_moves_to_rework() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (_repo, worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        fs::write(worktree.join("dirty.txt"), "dirty\n").expect("dirty worktree");

        let result =
            integrate_local_worktree(&pool, "TASK-1", &tasker_db::Actor::operator("tester"))
                .await
                .expect("integrate");

        assert!(result.summary.contains("work-change Delivery Failure"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "rework");
        let outcomes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM integration_outcomes WHERE outcome_kind = 'work_change_failure'",
        )
        .fetch_one(&pool)
        .await
        .expect("outcome count");
        assert_eq!(outcomes, 1);
    }

    #[tokio::test]
    async fn merge_integrate_allows_current_validated_base_without_branch_ancestry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        fs::write(repo.join("main-only.txt"), "main moved\n").expect("main change");
        git(&repo, &["add", "main-only.txt"]);
        git(&repo, &["commit", "-m", "move main"]);
        let current_main = git_output(&repo, &["rev-parse", "main"])
            .expect("main head")
            .trim()
            .to_string();
        tasker_db::update_validation_item_status(
            &pool,
            "TASK-1",
            1,
            &tasker_db::UpdateRequirementStatus {
                status: "passed".to_string(),
                waiver_reason: None,
                validated_base_commit: Some(current_main.clone()),
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("record validated base");

        let result =
            integrate_local_worktree(&pool, "TASK-1", &tasker_db::Actor::operator("tester"))
                .await
                .expect("integrate");

        assert!(result.summary.contains("Final Commit"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "done");
        assert_eq!(
            detail.task.validated_base_commit.as_deref(),
            Some(current_main.as_str())
        );
    }

    #[tokio::test]
    async fn merge_integrate_stale_validated_base_moves_to_rework() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        let old_main = git_output(&repo, &["rev-parse", "main"])
            .expect("main head")
            .trim()
            .to_string();
        tasker_db::update_validation_item_status(
            &pool,
            "TASK-1",
            1,
            &tasker_db::UpdateRequirementStatus {
                status: "passed".to_string(),
                waiver_reason: None,
                validated_base_commit: Some(old_main),
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("record stale base");
        fs::write(repo.join("main-only.txt"), "main moved\n").expect("main change");
        git(&repo, &["add", "main-only.txt"]);
        git(&repo, &["commit", "-m", "move main"]);

        let result =
            integrate_local_worktree(&pool, "TASK-1", &tasker_db::Actor::operator("tester"))
                .await
                .expect("integrate");

        assert!(result.summary.contains("work-change Delivery Failure"));
        assert!(result.summary.contains("Validated Base Commit"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "rework");
    }

    #[tokio::test]
    async fn merge_integrate_stale_task_branch_moves_to_rework() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        fs::write(repo.join("main-only.txt"), "main moved\n").expect("main change");
        git(&repo, &["add", "main-only.txt"]);
        git(&repo, &["commit", "-m", "move main"]);

        let result =
            integrate_local_worktree(&pool, "TASK-1", &tasker_db::Actor::operator("tester"))
                .await
                .expect("integrate");

        assert!(result.summary.contains("work-change Delivery Failure"));
        assert!(result
            .summary
            .contains("does not include current Main Branch"));
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "rework");
    }

    #[tokio::test]
    async fn merge_integrate_commit_failure_rolls_back_main_branch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let (repo, _worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;
        let pre_head = git_output(&repo, &["rev-parse", "HEAD"]).expect("head");
        let hooks = repo.join(".git/hooks");
        fs::write(hooks.join("pre-commit"), "#!/bin/sh\nexit 1\n").expect("hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(hooks.join("pre-commit"))
                .expect("hook metadata")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(hooks.join("pre-commit"), permissions).expect("chmod hook");
        }

        let result =
            integrate_local_worktree(&pool, "TASK-1", &tasker_db::Actor::operator("tester"))
                .await
                .expect("integrate");

        assert!(result.summary.contains("Final Commit failed"));
        let after_head = git_output(&repo, &["rev-parse", "HEAD"]).expect("head");
        assert_eq!(pre_head, after_head);
        assert!(git_output(&repo, &["status", "--porcelain"])
            .expect("status")
            .trim()
            .is_empty());
        let detail = tasker_db::get_task_detail(&pool, "TASK-1")
            .await
            .expect("task")
            .expect("task");
        assert_eq!(detail.task.state, "rework");
    }

    async fn seed_integrating_local_task(
        pool: &sqlx::SqlitePool,
        root: &Path,
        with_feature_commit: bool,
        done_worktree_retention: bool,
    ) -> (PathBuf, PathBuf) {
        let repo = root.join("repo");
        let worktrees = root.join("worktrees");
        let worktree = worktrees.join("TASK-1");
        init_git_repo(&repo);
        tasker_db::create_task_queue(
            pool,
            &tasker_db::CreateTaskQueue {
                key: "TASK".to_string(),
                name: "Tasker".to_string(),
                managed_source_repository: repo.display().to_string(),
                main_branch: "main".to_string(),
                worktree_root: worktrees.display().to_string(),
                branch_template: "tasker/{task_identifier}".to_string(),
                done_worktree_retention,
                queue_concurrency_limit: None,
            },
            &tasker_db::Actor::operator("tester"),
        )
        .await
        .expect("queue");
        tasker_db::create_task(
            pool,
            &tasker_db::CreateTask {
                queue_key: "TASK".to_string(),
                title: "Integrate me".to_string(),
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
        .expect("task");
        git(&repo, &["branch", "tasker/TASK-1", "main"]);
        fs::create_dir_all(&worktrees).expect("worktrees");
        git(
            &repo,
            &[
                "worktree",
                "add",
                worktree.to_str().expect("utf8"),
                "tasker/TASK-1",
            ],
        );
        if with_feature_commit {
            fs::write(worktree.join("feature.txt"), "feature\n").expect("feature");
            git(&worktree, &["add", "feature.txt"]);
            git(&worktree, &["commit", "-m", "add feature"]);
        }
        let actor = tasker_db::Actor::operator("tester");
        tasker_db::upsert_task_link(
            pool,
            "TASK-1",
            &tasker_db::UpsertTaskLink {
                kind: "local_worktree".to_string(),
                target: worktree.display().to_string(),
                label: Some("Local Worktree".to_string()),
                is_primary: true,
            },
            &actor,
        )
        .await
        .expect("worktree link");
        tasker_db::upsert_task_link(
            pool,
            "TASK-1",
            &tasker_db::UpsertTaskLink {
                kind: "task_branch".to_string(),
                target: "tasker/TASK-1".to_string(),
                label: Some("Task Branch".to_string()),
                is_primary: false,
            },
            &actor,
        )
        .await
        .expect("branch link");
        tasker_db::update_acceptance_criterion_status(
            pool,
            "TASK-1",
            1,
            &tasker_db::UpdateRequirementStatus {
                status: "satisfied".to_string(),
                waiver_reason: None,
                validated_base_commit: None,
            },
            &actor,
        )
        .await
        .expect("criterion");
        tasker_db::update_validation_item_status(
            pool,
            "TASK-1",
            1,
            &tasker_db::UpdateRequirementStatus {
                status: "passed".to_string(),
                waiver_reason: None,
                validated_base_commit: None,
            },
            &actor,
        )
        .await
        .expect("validation");
        tasker_db::transition_task_state(
            pool,
            "TASK-1",
            &tasker_db::TransitionTaskState {
                to_state: "in_progress".to_string(),
                agent_run_id: None,
            },
            &actor,
        )
        .await
        .expect("in progress");
        tasker_db::transition_task_state(
            pool,
            "TASK-1",
            &tasker_db::TransitionTaskState {
                to_state: "integrating".to_string(),
                agent_run_id: None,
            },
            &actor,
        )
        .await
        .expect("integrating");
        (repo, worktree)
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

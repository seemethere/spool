use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;
use tasker_config::{ensure_data_dir, PathOverrides, TaskerConfig, TaskerPaths};

mod bootstrap;
mod cleanup;
mod cleanup_cmd;
mod db_cmd;
mod delegate_cmd;
mod display;
mod merge_cmd;
mod monitor;
mod output;
mod queue_cmd;
mod review_cmd;
mod review_packet;
mod run_cmd;
mod serve_cmd;
mod status_cmd;
mod supervise_cmd;
mod task_cmd;
mod telemetry;
mod telemetry_cmd;
mod work_cmd;

use cleanup_cmd::cleanup;
use db_cmd::{db, init};
#[cfg(test)]
use db_cmd::{
    guard_db_migrate_source_from, guard_supervisor_auto_migrate_source_from,
    prepare_supervisor_migrations,
};
use delegate_cmd::delegate;
use merge_cmd::{git_output, merge};
#[cfg(test)]
use merge_cmd::{manual_squash_integration_guidance, post_merge_batch_validation_guidance};
use queue_cmd::queue;
use review_cmd::review;
use review_packet::review_packet;
use run_cmd::run;
use serve_cmd::serve;
#[cfg(test)]
use status_cmd::build_status_telemetry;
use status_cmd::{monitor, status};
use supervise_cmd::supervise;
#[cfg(test)]
use supervise_cmd::{default_worker_command, resolved_database_path};
use task_cmd::task;
use telemetry_cmd::telemetry;
use work_cmd::work;

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
    /// Manage Tasker database schema migrations.
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
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
    Status {
        /// Emit machine-readable lifecycle telemetry JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show Workflow Metric telemetry summaries.
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommand,
    },
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
    /// Supervise tasker work --once workers.
    #[command(
        after_long_help = "Supervisor progress logs are intentionally compact for unattended batches. Use `tasker status` or `tasker monitor` when you want human-readable Task titles and richer context."
    )]
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
        /// Keep polling for newly eligible Tasks until timeout or interruption.
        #[arg(long)]
        watch: bool,
        /// Agent Launcher passed to default tasker work --once workers.
        #[arg(long, default_value = "pi")]
        launcher: String,
        /// Worker command prefix for tests/debugging; --actor is appended automatically.
        #[arg(long, value_delimiter = ' ')]
        worker_command: Option<Vec<String>>,
        /// Intentionally bypass the per-Task Queue supervisor lock.
        #[arg(long)]
        allow_overlap: bool,
        /// Apply pending SQLite migrations before polling only when idle and on trusted Main Branch.
        #[arg(long)]
        auto_migrate_when_idle: bool,
    },
    /// Inspect Agent Runs.
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
    /// Build a read-only local Review Packet summary for a Human Review Task.
    ReviewPacket { identifier: String },
    /// Start a Pi-backed interactive Review Session for a Human Review Task.
    Review {
        /// Task Identifier.
        identifier: String,
        /// Review Agent actor display name.
        #[arg(long, default_value = "local-review-agent", hide = true)]
        actor: String,
        /// Tasker API URL exposed to the launched Review Agent.
        #[arg(long, hide = true)]
        api_url: Option<String>,
        /// Pi executable path.
        #[arg(long, default_value = "pi", hide = true)]
        pi_bin: String,
        /// Tasker Pi Extension file to load into pi.
        #[arg(long, hide = true)]
        pi_extension: Option<PathBuf>,
    },
    /// Start a Pi-backed interactive Delegation Session.
    Delegate {
        /// Task Queue Key for creating a new Root Task.
        #[arg(long)]
        queue: Option<String>,
        /// Refine an existing Backlog Task instead of creating a Root Task.
        #[arg(long)]
        refine: Option<String>,
        /// Delegating Agent actor display name.
        #[arg(long, default_value = "local-delegating-agent", hide = true)]
        actor: String,
        /// Tasker API URL exposed to the launched Delegating Agent.
        #[arg(long, hide = true)]
        api_url: Option<String>,
        /// Pi executable path.
        #[arg(long, default_value = "pi", hide = true)]
        pi_bin: String,
        /// Tasker Pi Extension file to load into pi.
        #[arg(long, hide = true)]
        pi_extension: Option<PathBuf>,
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
enum DbCommand {
    /// Apply pending SQLite migrations from the trusted Managed Source Repository Main Branch.
    Migrate {
        /// Allow migration from a Local Worktree or Task Branch after explicit operator verification.
        #[arg(long)]
        allow_task_branch: bool,
    },
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
        /// Use temporary Bootstrap Task Creation compatibility mode with --file.
        #[arg(long)]
        bootstrap: bool,
        /// Task Queue Key for the new Task.
        #[arg(long)]
        queue: String,
        /// Preferred file-backed Task definition with YAML front matter and the Task Brief body.
        #[arg(long = "from-file", value_name = "FILE", conflicts_with = "file")]
        from_file: Option<PathBuf>,
        /// Compatibility alias for Bootstrap Task Creation. Prefer --from-file for new usage.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Operator actor display name for audit attribution.
        #[arg(long, default_value = "local-operator")]
        actor: String,
    },
    /// Validate a file-backed Task definition without creating a Task.
    Lint {
        /// Markdown file containing YAML front matter and the Task Brief body.
        #[arg(long)]
        file: PathBuf,
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
        /// Operator-only Repair Override for exceptional gate repair.
        #[arg(long)]
        repair_override: bool,
    },
    /// Record a human Review Decision for a Task in Human Review.
    ReviewDecision {
        /// Task Identifier.
        identifier: String,
        /// Review Decision: approve or rework.
        #[arg(long)]
        decision: String,
        /// Human feedback text, required for rework decisions unless --feedback-file is used.
        #[arg(long, conflicts_with = "feedback_file")]
        feedback: Option<String>,
        /// File containing human feedback, required for rework decisions unless --feedback is used.
        #[arg(long)]
        feedback_file: Option<PathBuf>,
        /// Actor kind for audit attribution and permission checks.
        #[arg(long, default_value = "review_agent")]
        actor_kind: String,
        /// Actor display name for audit attribution.
        #[arg(long, default_value = "local-review-agent")]
        actor: String,
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
enum TelemetryCommand {
    /// Refresh normalized Agent Run metrics from local Run Transcripts and Launcher Session Data.
    BackfillMetrics {
        /// Optional Task Queue Key to restrict the backfill.
        #[arg(long)]
        queue: Option<String>,
        /// Persist refreshed metrics. Without this flag the command only reports planned changes.
        #[arg(long)]
        write: bool,
        /// Emit machine-readable backfill telemetry JSON.
        #[arg(long)]
        json: bool,
    },
    /// Summarize Agent Run waste, latency, and sanitized local proxy efficiency metrics for a Task Queue.
    Summary {
        /// Task Queue Key to summarize.
        #[arg(long)]
        queue: String,
        /// Number of slow completed Agent Runs to list.
        #[arg(long, default_value_t = 5)]
        slow_limit: usize,
        /// Efficiency budget source: fixed or adaptive. Defaults to config telemetry.efficiency_budget.
        #[arg(long, value_parser = ["fixed", "adaptive"])]
        efficiency_budget: Option<String>,
        /// Recent Agent Run window for adaptive efficiency budgets.
        #[arg(long = "adaptive-budget-window", value_parser = clap::value_parser!(u64).range(1..))]
        adaptive_budget_window: Option<u64>,
        /// Emit machine-readable Agent Run telemetry JSON.
        #[arg(long)]
        json: bool,
    },
    /// Summarize Task lifecycle latency from Task State transition Audit Events.
    Lifecycle {
        /// Optional Task Queue Key filter.
        #[arg(long)]
        queue: Option<String>,
        /// Number of recent slowest Tasks to include.
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Correlate Agent Run telemetry before/after dogfood fix landing points.
    Correlation {
        /// Task Queue Key to summarize.
        #[arg(long)]
        queue: String,
        /// Fix landing point from a completed Task Identifier.
        #[arg(long = "landing-task")]
        landing_tasks: Vec<String>,
        /// Fix landing point from an Integration Outcome final commit SHA.
        #[arg(long = "landing-commit")]
        landing_commits: Vec<String>,
        /// Explicit fix landing point timestamp (SQLite-compatible UTC string).
        #[arg(long = "landing-at")]
        landing_timestamps: Vec<String>,
        /// Emit machine-readable correlation telemetry JSON.
        #[arg(long)]
        json: bool,
    },
    /// Compare sanitized local proxy Agent Run efficiency trends around dogfood landing points.
    Trend {
        /// Task Queue Key to summarize.
        #[arg(long)]
        queue: String,
        /// Landing point from a completed Task Identifier.
        #[arg(long = "landing-task")]
        landing_tasks: Vec<String>,
        /// Explicit landing point timestamp (SQLite-compatible UTC string).
        #[arg(long = "landing-at")]
        landing_timestamps: Vec<String>,
        /// Number of Agent Runs before each landing point to include.
        #[arg(long, default_value_t = 10)]
        before_runs: usize,
        /// Number of Agent Runs after each landing point to include.
        #[arg(long, default_value_t = 10)]
        after_runs: usize,
        /// Emit machine-readable trend telemetry JSON.
        #[arg(long)]
        json: bool,
    },
    /// Compact recent Agent Run efficiency report for dogfood tuning.
    Efficiency {
        /// Task Queue Key to summarize.
        #[arg(long)]
        queue: String,
        /// Number of latest Agent Runs to include in the recent window.
        #[arg(long, default_value_t = 20)]
        recent: usize,
        /// Number of top offender Agent Runs to list.
        #[arg(long, default_value_t = 5)]
        top_limit: usize,
        /// Emit machine-readable recent efficiency telemetry JSON.
        #[arg(long)]
        json: bool,
    },
    /// Aggregate completed pi Agent Run metrics into a local Workflow Metrics report.
    Workflow {
        /// Task Queue Key to summarize.
        #[arg(long)]
        queue: String,
        /// Number of most recent completed pi Agent Runs to include. Ignored when --since or --until is set.
        #[arg(long, default_value_t = 20)]
        recent: usize,
        /// Start of created_at date window, as a SQLite-compatible UTC timestamp.
        #[arg(long)]
        since: Option<String>,
        /// End of created_at date window, as a SQLite-compatible UTC timestamp.
        #[arg(long)]
        until: Option<String>,
        /// Emit machine-readable Workflow Metrics JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum RunCommand {
    /// Show one Agent Run, its Task, and Launcher Session Data.
    Show {
        run_id: String,
        /// Emit machine-readable Agent Run telemetry JSON.
        #[arg(long)]
        json: bool,
    },
    /// Operator recovery: fail an active Agent Run and create a Retry Hold.
    Fail {
        /// Agent Run ID.
        run_id: String,
        /// Explicit reason recorded on the Agent Run and Retry Hold.
        #[arg(long)]
        reason: String,
        /// Structured failure reason code. Defaults to operator_failed.
        #[arg(long)]
        failure_reason_code: Option<String>,
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
    /// Summarize or remove Done/Canceled Local Worktrees and Task Branches.
    LocalWorktrees {
        /// Task Queue Key whose Local Worktree Delivery configuration should be used.
        #[arg(long)]
        queue: String,
        /// Explicitly keep files/branches and only report cleanup candidates.
        #[arg(long, conflicts_with = "delete")]
        dry_run: bool,
        /// Delete verified safe Local Worktrees and Task Branches.
        #[arg(long)]
        delete: bool,
    },
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
    /// Manage the queue-scoped Managed Source Repository operation lock.
    Lock {
        #[command(subcommand)]
        command: MergeLockCommand,
    },
    /// Integrate an already-Integrating Task through runner-side Local Worktree Delivery.
    Integrate {
        /// Task Identifier.
        identifier: String,
        /// Operator actor display name for audit attribution.
        #[arg(long, default_value = "local-operator")]
        actor: String,
    },
    /// Retry Local Worktree Delivery for an Integrating Task without launching a new Agent Run.
    #[command(
        after_long_help = "Use this operator recovery command when an Integrating Task's latest Integration Outcome is a retryable operational_failure and the local operational issue has been fixed. It re-runs only the Delivery Adapter path; it does not claim work, launch a new Agent Run, or re-run Worker Agent code. Use `tasker task retry` for failed/stuck agent work that should return to Ready, and use Rework for work_change_failure outcomes that require Task changes."
    )]
    Retry {
        /// Task Identifier.
        identifier: String,
        /// Bypass Task State/latest Integration Outcome safety checks after operator verification.
        #[arg(long)]
        force: bool,
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

#[derive(Debug, Subcommand)]
enum MergeLockCommand {
    /// Acquire a manual Managed Source Repository operation lock.
    Acquire {
        /// Task Queue Key whose Managed Source Repository should be protected.
        #[arg(long)]
        queue: String,
        /// Operation description recorded in the lock file.
        #[arg(long, default_value = "manual_integration")]
        operation: String,
        /// Optional Task Identifier associated with the manual operation.
        #[arg(long)]
        task: Option<String>,
    },
    /// Show the current Managed Source Repository operation lock for a queue.
    Status {
        /// Task Queue Key.
        #[arg(long)]
        queue: String,
    },
    /// Release a manual Managed Source Repository operation lock after operator verification.
    Release {
        /// Task Queue Key.
        #[arg(long)]
        queue: String,
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
    watch: bool,
    auto_migrate_when_idle: bool,
}

struct ReviewOptions {
    identifier: String,
    actor: String,
    api_url: Option<String>,
    pi_bin: String,
    pi_extension: Option<PathBuf>,
}

struct DelegateOptions {
    queue: Option<String>,
    refine: Option<String>,
    actor: String,
    api_url: Option<String>,
    pi_bin: String,
    pi_extension: Option<PathBuf>,
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
        Some(Command::Db { command }) => db(&paths, db_path_overridden, command).await,
        Some(Command::Queue { command }) => queue(&paths, db_path_overridden, command).await,
        Some(Command::Task { command }) => task(&paths, db_path_overridden, command).await,
        Some(Command::Status { json }) => status(&paths, db_path_overridden, json).await,
        Some(Command::Telemetry { command }) => {
            telemetry(&paths, db_path_overridden, command).await
        }
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
            watch,
            launcher,
            worker_command,
            allow_overlap,
            auto_migrate_when_idle,
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
                    watch,
                    auto_migrate_when_idle,
                },
            )
            .await
        }
        Some(Command::Run { command }) => run(&paths, db_path_overridden, command).await,
        Some(Command::ReviewPacket { identifier }) => {
            review_packet(&paths, db_path_overridden, identifier).await
        }
        Some(Command::Review {
            identifier,
            actor,
            api_url,
            pi_bin,
            pi_extension,
        }) => {
            review(
                &paths,
                db_path_overridden,
                ReviewOptions {
                    identifier,
                    actor,
                    api_url,
                    pi_bin,
                    pi_extension,
                },
            )
            .await
        }
        Some(Command::Delegate {
            queue,
            refine,
            actor,
            api_url,
            pi_bin,
            pi_extension,
        }) => {
            delegate(
                &paths,
                db_path_overridden,
                DelegateOptions {
                    queue,
                    refine,
                    actor,
                    api_url,
                    pi_bin,
                    pi_extension,
                },
            )
            .await
        }
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
        CleanupCommand::LocalWorktrees { delete, .. }
        | CleanupCommand::CargoTargets { delete, .. }
        | CleanupCommand::Runs { delete, .. } => *delete,
    }
}

fn command_is_unsafe_mutation(command: &Option<Command>) -> bool {
    match command {
        Some(
            Command::Init
            | Command::Db { .. }
            | Command::Work { .. }
            | Command::Supervise { .. }
            | Command::Review { .. }
            | Command::Delegate { .. }
            | Command::Serve { .. },
        ) => true,
        Some(Command::Queue { command }) => matches!(
            command,
            QueueCommand::Create { .. } | QueueCommand::Update { .. }
        ),
        Some(Command::Task { command }) => !matches!(
            command,
            TaskCommand::Lint { .. } | TaskCommand::Show { .. } | TaskCommand::Audit { .. }
        ),
        Some(Command::Run { command }) => matches!(command, RunCommand::Fail { .. }),
        Some(Command::Cleanup { command }) => cleanup_command_is_unsafe_mutation(command),
        Some(Command::Merge { command }) => {
            matches!(
                command,
                MergeCommand::Integrate { .. }
                    | MergeCommand::Retry { .. }
                    | MergeCommand::Done { .. }
                    | MergeCommand::Lock {
                        command: MergeLockCommand::Acquire { .. }
                            | MergeLockCommand::Release { .. }
                    }
            )
        }
        Some(
            Command::Status { .. }
            | Command::Telemetry { .. }
            | Command::Monitor { .. }
            | Command::ReviewPacket { .. }
            | Command::Version,
        )
        | None => false,
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
        Some(Command::Db { .. }) => None,
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
            | TaskCommand::ReviewDecision { identifier, .. }
            | TaskCommand::Audit { identifier } => queue_key_from_task_identifier(identifier),
            TaskCommand::Criterion { command } | TaskCommand::Validation { command } => {
                requirement_command_queue_key(command)
            }
            TaskCommand::Workpad { command } => workpad_command_queue_key(command),
            TaskCommand::Lint { .. } | TaskCommand::Show { .. } => None,
        },
        Some(
            Command::Work { queue, .. }
            | Command::Supervise { queue, .. }
            | Command::Monitor {
                queue: Some(queue), ..
            },
        ) => Some(queue.clone()),
        Some(Command::Telemetry { command }) => match command {
            TelemetryCommand::Summary { queue, .. }
            | TelemetryCommand::Correlation { queue, .. }
            | TelemetryCommand::Trend { queue, .. }
            | TelemetryCommand::Efficiency { queue, .. }
            | TelemetryCommand::Workflow { queue, .. } => Some(queue.clone()),
            TelemetryCommand::Lifecycle { queue, .. }
            | TelemetryCommand::BackfillMetrics { queue, .. } => queue.clone(),
        },
        Some(Command::Monitor { queue: None, .. } | Command::Cleanup { .. }) => None,
        Some(Command::ReviewPacket { identifier }) => queue_key_from_task_identifier(identifier),
        Some(Command::Review { identifier, .. }) => queue_key_from_task_identifier(identifier),
        Some(Command::Delegate { queue, refine, .. }) => refine
            .as_deref()
            .and_then(queue_key_from_task_identifier)
            .or_else(|| queue.clone()),
        Some(Command::Merge { command }) => match command {
            MergeCommand::Queue { queue } => queue.clone(),
            MergeCommand::Inspect { .. } => None,
            MergeCommand::Integrate { identifier, .. }
            | MergeCommand::Retry { identifier, .. }
            | MergeCommand::Done { identifier, .. } => queue_key_from_task_identifier(identifier),
            MergeCommand::Lock { command } => match command {
                MergeLockCommand::Acquire { queue, .. }
                | MergeLockCommand::Status { queue }
                | MergeLockCommand::Release { queue } => Some(queue.clone()),
            },
        },
        Some(Command::Init | Command::Run { .. } | Command::Serve { .. })
        | Some(Command::Status { .. } | Command::Version)
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

async fn open_pool(paths: &TaskerPaths, db_path_overridden: bool) -> Result<sqlx::SqlitePool> {
    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    let pool = tasker_db::connect(&config.database.path).await?;
    tasker_db::check_migration_compatibility(&pool).await?;
    Ok(pool)
}

fn git_status(repo: &Path, args: &[&str]) -> Result<std::process::ExitStatus> {
    std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .with_context(|| format!("run git {} in {}", args.join(" "), repo.display()))
}

fn ensure_db_parent(db_path: &Path) -> Result<()> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod cli_tests;

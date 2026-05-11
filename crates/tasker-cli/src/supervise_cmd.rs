use super::*;
use crate::db_cmd::{manual_migration_command, prepare_supervisor_migrations};

pub(crate) async fn supervise(
    paths: &TaskerPaths,
    db_path_overridden: bool,
    path_forwarding: PathForwardingOptions,
    options: SuperviseOptions,
) -> Result<()> {
    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    let pool = tasker_db::connect(&config.database.path).await?;
    let manual_command = manual_migration_command(paths, db_path_overridden);
    if !prepare_supervisor_migrations(&pool, &options, &manual_command).await? {
        return Ok(());
    }
    tasker_db::check_migration_compatibility(&pool).await?;
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
            data_dir: paths.data_dir.clone(),
            allow_overlap: options.allow_overlap,
            watch: options.watch,
            #[cfg(test)]
            run_prefix: None,
        },
    )
    .await?;

    println!(
        "supervisor summary: started={} completed={} failed={} no_eligible={} completed_handoffs={} blocked_reports={} retryable_failures={} stuck_runs={} repo_lock_blocks={} timed_out={}",
        outcome.started_workers,
        outcome.completed_workers,
        outcome.failed_workers,
        outcome.no_eligible_exits,
        outcome.completed_handoffs,
        outcome.blocked_reports,
        outcome.retryable_failure_reports,
        outcome.stuck_runs.len(),
        outcome.repo_operation_lock_blocks,
        outcome.timed_out
    );
    Ok(())
}

pub(crate) fn default_worker_command(
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

pub(crate) fn resolved_database_path(
    paths: &TaskerPaths,
    db_path_overridden: bool,
) -> Result<PathBuf> {
    let mut config = TaskerConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    Ok(config.database.path)
}

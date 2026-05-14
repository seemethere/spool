use super::*;

pub(crate) async fn init(paths: &SpoolPaths, db_path_overridden: bool) -> Result<()> {
    ensure_data_dir(paths)?;

    let mut config = SpoolConfig::load_or_default(paths)?;
    if db_path_overridden {
        config.database.path = paths.db_path.clone();
    }
    let wrote_config = config.write_if_missing(paths)?;
    ensure_db_parent(&config.database.path)?;

    let pool = spool_db::connect(&config.database.path).await?;
    spool_db::run_migrations(&pool).await?;
    let token = spool_db::ensure_local_api_token(&pool).await?;

    println!("Spool initialized");
    println!("config: {}", paths.config_path.display());
    println!("data: {}", paths.data_dir.display());
    println!("database: {}", config.database.path.display());
    println!("local api token: {token}");
    if !wrote_config {
        println!("config already existed; left unchanged");
    }

    Ok(())
}

pub(crate) async fn db(
    paths: &SpoolPaths,
    db_path_overridden: bool,
    command: DbCommand,
) -> Result<()> {
    match command {
        DbCommand::Migrate { allow_task_branch } => {
            let mut config = SpoolConfig::load_or_default(paths)?;
            if db_path_overridden {
                config.database.path = paths.db_path.clone();
            }
            ensure_db_parent(&config.database.path)?;
            let pool = spool_db::connect(&config.database.path).await?;
            guard_db_migrate_source(&pool, allow_task_branch).await?;
            spool_db::run_migrations(&pool).await?;
            let token = spool_db::ensure_local_api_token(&pool).await?;
            println!("Spool database migrated");
            println!("database: {}", config.database.path.display());
            println!("local api token: {token}");
        }
    }
    Ok(())
}

async fn guard_db_migrate_source(pool: &sqlx::SqlitePool, allow_task_branch: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
    guard_db_migrate_source_from(pool, allow_task_branch, &cwd).await
}

pub(crate) async fn guard_db_migrate_source_from(
    pool: &sqlx::SqlitePool,
    allow_task_branch: bool,
    cwd: &Path,
) -> Result<()> {
    if allow_task_branch {
        return Ok(());
    }

    let queues = spool_db::list_task_queues(pool).await.unwrap_or_default();
    if queues.is_empty() {
        return Ok(());
    }

    let cwd_git_root = git_output(cwd, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(|output| PathBuf::from(output.trim()));
    for queue in queues {
        let repo = PathBuf::from(&queue.managed_source_repository);
        if !cwd_git_root
            .as_ref()
            .is_some_and(|root| paths_equivalent(root, &repo))
        {
            anyhow::bail!(
                "refusing to migrate the Task Backend from {} because Task Queue {} is configured for Managed Source Repository {}. Run `spool db migrate` from the Managed Source Repository Main Branch after integration, or pass --allow-task-branch only after explicit operator verification.",
                cwd.display(),
                queue.key,
                repo.display()
            );
        }

        let branch = git_output(&repo, &["branch", "--show-current"])?;
        let branch = branch.trim();
        if branch != queue.main_branch {
            anyhow::bail!(
                "refusing to migrate the Task Backend from Git branch {branch}; Task Queue {} requires Managed Source Repository Main Branch {}. Switch to Main Branch and rerun `spool db migrate`, or pass --allow-task-branch only after explicit operator verification.",
                queue.key,
                queue.main_branch
            );
        }
    }

    Ok(())
}

pub(crate) async fn prepare_supervisor_migrations(
    pool: &sqlx::SqlitePool,
    options: &SuperviseOptions,
    manual_command: &str,
) -> Result<bool> {
    let pending = spool_db::pending_migration_versions(pool).await?;
    if pending.is_empty() {
        return Ok(true);
    }

    let active_runs = active_agent_run_count(pool).await?;
    let safe = active_runs == 0;
    println!(
        "supervisor migration-required pause for Task Queue {}; no worker started and no Agent Run was created",
        options.queue
    );
    println!("pending SQLite migrations: {pending:?}");
    println!("active Agent Runs: {active_runs}");
    println!("migration currently safe: {safe}");
    println!("manual migration command: {manual_command}");

    if !options.auto_migrate_when_idle {
        println!(
            "auto-migrate-when-idle: disabled; rerun from the trusted Managed Source Repository Main Branch with --auto-migrate-when-idle, or run `{manual_command}` manually"
        );
        return Ok(false);
    }

    if active_runs > 0 {
        println!(
            "auto-migrate-when-idle refused because active Agent Runs exist; wait for completion or inspect with `spool status` before running `{manual_command}`"
        );
        return Ok(false);
    }

    guard_supervisor_auto_migrate_source(pool).await?;
    println!("auto-migrate-when-idle: applying pending SQLite migrations {pending:?}");
    spool_db::run_migrations(pool).await?;
    println!("auto-migrate-when-idle: Spool database migrated");
    Ok(true)
}

pub(crate) fn manual_migration_command(paths: &SpoolPaths, db_path_overridden: bool) -> String {
    let mut parts = vec![
        "spool".to_string(),
        "--config".to_string(),
        paths.config_path.display().to_string(),
        "--data-dir".to_string(),
        paths.data_dir.display().to_string(),
    ];
    if db_path_overridden {
        parts.push("--db-path".to_string());
        parts.push(paths.db_path.display().to_string());
    }
    parts.push("db".to_string());
    parts.push("migrate".to_string());
    parts.join(" ")
}

pub(crate) async fn active_agent_run_count(pool: &sqlx::SqlitePool) -> Result<i64> {
    let table_exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'agent_runs'",
    )
    .fetch_one(pool)
    .await
    .context("failed to inspect Agent Run table")?;
    if table_exists == 0 {
        return Ok(0);
    }

    sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_runs WHERE outcome IS NULL AND lease_expires_at > CURRENT_TIMESTAMP",
    )
    .fetch_one(pool)
    .await
    .context("failed to count active Agent Runs")
}

pub(crate) async fn guard_supervisor_auto_migrate_source(pool: &sqlx::SqlitePool) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
    guard_supervisor_auto_migrate_source_from(pool, &cwd).await
}

pub(crate) async fn guard_supervisor_auto_migrate_source_from(
    pool: &sqlx::SqlitePool,
    cwd: &Path,
) -> Result<()> {
    let queues = spool_db::list_task_queues(pool).await.unwrap_or_default();
    if queues.is_empty() {
        return Ok(());
    }

    let cwd_git_root = git_output(cwd, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(|output| PathBuf::from(output.trim()));
    for queue in queues {
        let repo = PathBuf::from(&queue.managed_source_repository);
        if !cwd_git_root
            .as_ref()
            .is_some_and(|root| paths_equivalent(root, &repo))
        {
            anyhow::bail!(
                "auto-migrate-when-idle refused from {} because Task Queue {} is configured for Managed Source Repository {}. Switch to the trusted Managed Source Repository Main Branch and run `spool db migrate`, or restart supervisor there with --auto-migrate-when-idle.",
                cwd.display(),
                queue.key,
                repo.display()
            );
        }

        let branch = git_output(&repo, &["branch", "--show-current"])?;
        let branch = branch.trim();
        if branch != queue.main_branch {
            anyhow::bail!(
                "auto-migrate-when-idle refused from Git branch {branch}; Task Queue {} requires Managed Source Repository Main Branch {}. Switch to Main Branch and run `spool db migrate`, or restart supervisor there with --auto-migrate-when-idle.",
                queue.key,
                queue.main_branch
            );
        }

        let status = git_output(&repo, &["status", "--porcelain"])?;
        if !status.trim().is_empty() {
            anyhow::bail!(
                "auto-migrate-when-idle refused because Managed Source Repository {} is dirty or has unresolved changes. Clean or resolve the repository, then run `spool db migrate` or restart supervisor with --auto-migrate-when-idle.",
                repo.display()
            );
        }
    }

    Ok(())
}

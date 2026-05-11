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
fn supervise_help_points_title_seekers_to_status_and_monitor() {
    let mut command = Cli::command();
    let supervise = command
        .find_subcommand_mut("supervise")
        .expect("supervise subcommand");
    let help = supervise.render_long_help().to_string();

    assert!(help.contains("Supervisor progress logs are intentionally compact"));
    assert!(help.contains("tasker status"));
    assert!(help.contains("tasker monitor"));
    assert!(help.contains("Task titles"));
}

#[tokio::test]
async fn db_migrate_guard_refuses_local_worktree_checkout_by_default() {
    let temp = tempfile::tempdir().expect("tempdir");
    let main_repo = temp.path().join("repo");
    let worktree = temp.path().join("worktree");
    init_git_repo(&main_repo);
    git(
        &main_repo,
        &[
            "worktree",
            "add",
            "-b",
            "tasker/TASK-1",
            worktree.to_str().unwrap(),
        ],
    );

    let db_path = temp.path().join("tasker.db");
    let pool = tasker_db::connect(&db_path).await.expect("connect");
    tasker_db::run_migrations(&pool).await.expect("migrate");
    tasker_db::create_task_queue(
        &pool,
        &tasker_db::CreateTaskQueue {
            key: "TASK".to_string(),
            name: "Tasker".to_string(),
            managed_source_repository: main_repo.display().to_string(),
            main_branch: "main".to_string(),
            worktree_root: temp.path().join("worktrees").display().to_string(),
            branch_template: "tasker/{task_identifier}".to_string(),
            done_worktree_retention: false,
            queue_concurrency_limit: None,
        },
        &tasker_db::Actor::operator("operator"),
    )
    .await
    .expect("queue");

    let error = guard_db_migrate_source_from(&pool, false, &worktree)
        .await
        .expect_err("Local Worktree should be refused");
    assert!(error.to_string().contains("refusing to migrate"));
    assert!(error.to_string().contains("Managed Source Repository"));

    guard_db_migrate_source_from(&pool, false, &main_repo)
        .await
        .expect("Main Branch should be accepted");
}

#[tokio::test]
async fn open_pool_rejects_pending_migrations_before_work_can_claim() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&data_dir).expect("data dir");
    let db_path = data_dir.join("tasker.db");
    let paths = TaskerPaths {
        config_path,
        data_dir,
        db_path: db_path.clone(),
    };
    TaskerConfig::default_for_paths(&paths)
        .write_if_missing(&paths)
        .expect("write config");
    let pool = tasker_db::connect(&db_path).await.expect("connect");
    sqlx::query(
        r#"
            CREATE TABLE _sqlx_migrations (
                version BIGINT PRIMARY KEY,
                description TEXT NOT NULL,
                installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                success BOOLEAN NOT NULL,
                checksum BLOB NOT NULL,
                execution_time BIGINT NOT NULL
            )
            "#,
    )
    .execute(&pool)
    .await
    .expect("create migrations table");
    drop(pool);

    let error = open_pool(&paths, false)
        .await
        .expect_err("pending migrations should prevent Worker Loop setup");
    assert!(error.to_string().contains("pending SQLite migrations"));
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
fn merge_inspect_guidance_prefers_squash_and_tasker_authority() {
    let guidance = manual_squash_integration_guidance("TASKER", "TASKER-60").join("\n");

    assert!(guidance.contains("git merge --squash <task-branch>"));
    assert!(guidance.contains("Conventional Commit subject"));
    assert!(guidance.contains("TASKER-60"));
    assert!(guidance.contains("Do not use Task Branch ancestry as completion proof"));
    assert!(guidance.contains("Tasker DB state, Integration Outcomes, Audit Events, and the Final Commit are authoritative"));
}

#[test]
fn status_json_exposes_lifecycle_telemetry_shape() {
    let rows = vec![
        tasker_db::QueueStatus {
            queue_key: "TASK".to_string(),
            queue_name: "Tasker".to_string(),
            queue_concurrency_limit: Some(1),
            state: "ready".to_string(),
            task_count: 2,
            ready_tasks: 2,
            integrating_tasks: 1,
            active_agent_runs: 1,
            active_integrating_agent_runs: 1,
            active_retry_holds: 1,
        },
        tasker_db::QueueStatus {
            queue_key: "TASK".to_string(),
            queue_name: "Tasker".to_string(),
            queue_concurrency_limit: Some(1),
            state: "integrating".to_string(),
            task_count: 1,
            ready_tasks: 2,
            integrating_tasks: 1,
            active_agent_runs: 1,
            active_integrating_agent_runs: 1,
            active_retry_holds: 1,
        },
    ];
    let active_runs = vec![tasker_db::ActiveAgentRunStatus {
        queue_key: "TASK".to_string(),
        task_identifier: "TASK-1".to_string(),
        task_title: "Run task".to_string(),
        task_state: "integrating".to_string(),
        agent_run_id: "run-1".to_string(),
        launcher_kind: "pi".to_string(),
        worker_id: "worker-1".to_string(),
        lease_expires_at: "later".to_string(),
    }];
    let active_holds = vec![tasker_db::ActiveRetryHoldStatus {
        queue_key: "TASK".to_string(),
        task_identifier: "TASK-2".to_string(),
        hold_until: "later".to_string(),
        reason: "retry".to_string(),
        failure_reason_code: Some("launcher_timeout".to_string()),
    }];
    let status_tasks = vec![tasker_db::TaskStatusSummary {
        queue_key: "TASK".to_string(),
        identifier: "TASK-3".to_string(),
        title: "Ready task".to_string(),
        state: "ready".to_string(),
        priority: "normal".to_string(),
        local_worktree: None,
        task_branch: None,
        main_branch: "main".to_string(),
        latest_rework_reason_code: None,
        latest_rework_reason: None,
        unresolved_blocking_task_count: 0,
        blocking_task_identifiers: None,
    }];
    let conflicts = vec![tasker_db::TaskConflictGroup {
        queue_key: "TASK".to_string(),
        target: "crates/tasker-cli".to_string(),
        task_count: 2,
        tasks: "TASK-3,TASK-4".to_string(),
    }];
    let integration_retries = vec![tasker_db::IntegrationRetryStatus {
        queue_key: "TASK".to_string(),
        task_identifier: "TASK-5".to_string(),
        task_title: "Retry integration".to_string(),
        reason_code: "dirty_managed_source_repository".to_string(),
        retryable: true,
        retry_attempt: Some(1),
        next_retry_at: Some("later".to_string()),
        reason: Some("dirty repo".to_string()),
    }];

    let value = serde_json::to_value(build_status_telemetry(
        &rows,
        &active_runs,
        &active_holds,
        &status_tasks,
        &conflicts,
        &integration_retries,
    ))
    .expect("status json");

    assert_eq!(value["queues"][0]["queue_key"], "TASK");
    assert_eq!(value["queues"][0]["available_claim_slots"], 0);
    assert_eq!(value["queues"][0]["capacity_saturated"], true);
    assert_eq!(value["queues"][0]["state_counts"][0]["state"], "ready");
    assert_eq!(
        value["queues"][0]["active_runs"][0]["agent_run_id"],
        "run-1"
    );
    assert_eq!(
        value["queues"][0]["ready_task_summaries"][0]["identifier"],
        "TASK-3"
    );
    assert_eq!(
        value["queues"][0]["advisory_conflict_hints"][0]["target"],
        "crates/tasker-cli"
    );
    assert_eq!(value["queues"][0]["retry_holds"][0]["reason"], "retry");
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

#[tokio::test]
async fn supervisor_migration_pause_reports_pending_without_claiming_tasks() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = TaskerPaths::resolve(temp.path().join("home"), PathOverrides::default());
    init(&paths, false).await.expect("init");
    let pool = open_pool(&paths, false).await.expect("pool");
    let repo = temp.path().join("repo");
    init_git_repo(&repo);
    tasker_db::create_task_queue(
        &pool,
        &tasker_db::CreateTaskQueue {
            key: "TASK".to_string(),
            name: "Tasker".to_string(),
            managed_source_repository: repo.display().to_string(),
            main_branch: "main".to_string(),
            worktree_root: temp.path().join("worktrees").display().to_string(),
            branch_template: "tasker/{task_identifier}".to_string(),
            done_worktree_retention: false,
            queue_concurrency_limit: None,
        },
        &tasker_db::Actor::operator("tester"),
    )
    .await
    .expect("queue");
    tasker_db::create_task(
        &pool,
        &tasker_db::CreateTask {
            queue_key: "TASK".to_string(),
            title: "Do work".to_string(),
            brief: "Brief".to_string(),
            priority: "normal".to_string(),
            state: "ready".to_string(),
            review_required: false,
            acceptance_criteria: vec!["accepted".to_string()],
            validation_items: vec!["validated".to_string()],
            tags: vec![],
            conflict_hints: vec![],
            blocking_task_identifiers: vec![],
        },
        &tasker_db::Actor::operator("tester"),
    )
    .await
    .expect("task");
    forget_applied_migrations(&pool).await;

    let should_continue = prepare_supervisor_migrations(
        &pool,
        &SuperviseOptions {
            queue: "TASK".to_string(),
            concurrency: 1,
            timeout_seconds: 60,
            poll_seconds: 5,
            launcher: "fake".to_string(),
            worker_command: None,
            allow_overlap: false,
            watch: false,
            auto_migrate_when_idle: false,
        },
        "tasker db migrate",
    )
    .await
    .expect("migration pause");

    assert!(!should_continue);
    assert!(!tasker_db::pending_migration_versions(&pool)
        .await
        .expect("pending")
        .is_empty());
    let active_runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs")
        .fetch_one(&pool)
        .await
        .expect("active runs");
    assert_eq!(active_runs, 0);
}

#[tokio::test]
async fn supervisor_auto_migrate_when_idle_applies_pending_migrations_with_zero_active_runs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("tasker.db");
    let pool = tasker_db::connect(&db_path).await.expect("connect");
    create_empty_migrations_table(&pool).await;

    let should_continue = prepare_supervisor_migrations(
        &pool,
        &SuperviseOptions {
            queue: "TASK".to_string(),
            concurrency: 1,
            timeout_seconds: 60,
            poll_seconds: 5,
            launcher: "fake".to_string(),
            worker_command: None,
            allow_overlap: false,
            watch: false,
            auto_migrate_when_idle: true,
        },
        "tasker db migrate",
    )
    .await
    .expect("auto migrate");

    assert!(should_continue);
    assert!(tasker_db::pending_migration_versions(&pool)
        .await
        .expect("pending")
        .is_empty());
}

#[tokio::test]
async fn supervisor_auto_migrate_when_idle_refuses_active_runs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = TaskerPaths::resolve(temp.path().join("home"), PathOverrides::default());
    let (pool, _worktree, _run_id) =
        seed_in_progress_local_task(&paths, temp.path(), true, false).await;
    forget_applied_migrations(&pool).await;

    let should_continue = prepare_supervisor_migrations(
        &pool,
        &SuperviseOptions {
            queue: "TASK".to_string(),
            concurrency: 1,
            timeout_seconds: 60,
            poll_seconds: 5,
            launcher: "fake".to_string(),
            worker_command: None,
            allow_overlap: false,
            watch: false,
            auto_migrate_when_idle: true,
        },
        "tasker db migrate",
    )
    .await
    .expect("active run refusal");

    assert!(!should_continue);
    assert!(!tasker_db::pending_migration_versions(&pool)
        .await
        .expect("pending")
        .is_empty());
}

#[tokio::test]
async fn supervisor_auto_migrate_guard_refuses_untrusted_or_dirty_checkouts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("tasker.db");
    let pool = tasker_db::connect(&db_path).await.expect("connect");
    tasker_db::run_migrations(&pool).await.expect("migrate");
    let repo = temp.path().join("repo");
    let worktree = temp.path().join("worktree");
    init_git_repo(&repo);
    tasker_db::create_task_queue(
        &pool,
        &tasker_db::CreateTaskQueue {
            key: "TASK".to_string(),
            name: "Tasker".to_string(),
            managed_source_repository: repo.display().to_string(),
            main_branch: "main".to_string(),
            worktree_root: temp.path().join("worktrees").display().to_string(),
            branch_template: "tasker/{task_identifier}".to_string(),
            done_worktree_retention: false,
            queue_concurrency_limit: None,
        },
        &tasker_db::Actor::operator("tester"),
    )
    .await
    .expect("queue");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "tasker/TASK-1",
            worktree.to_str().unwrap(),
        ],
    );
    let error = guard_supervisor_auto_migrate_source_from(&pool, &worktree)
        .await
        .expect_err("Local Worktree should be refused");
    assert!(error.to_string().contains("auto-migrate-when-idle refused"));

    git(&repo, &["checkout", "-b", "tasker/TASK-2"]);
    let error = guard_supervisor_auto_migrate_source_from(&pool, &repo)
        .await
        .expect_err("Task Branch should be refused");
    assert!(error
        .to_string()
        .contains("requires Managed Source Repository Main Branch"));

    git(&repo, &["checkout", "main"]);
    fs::write(repo.join("dirty.txt"), "dirty\n").expect("dirty");
    let error = guard_supervisor_auto_migrate_source_from(&pool, &repo)
        .await
        .expect_err("dirty repo should be refused");
    assert!(error
        .to_string()
        .contains("dirty or has unresolved changes"));
    fs::remove_file(repo.join("dirty.txt")).expect("clean");

    guard_supervisor_auto_migrate_source_from(&pool, &repo)
        .await
        .expect("clean Main Branch should be accepted");
}

#[test]
fn supervise_default_worker_command_forwards_project_config_and_child_infers_project_data_dir() {
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
            watch: false,
            auto_migrate_when_idle: false,
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
    assert_eq!(paths.data_dir, temp.path().join("repo/.tasker/data"));
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
            watch: false,
            auto_migrate_when_idle: false,
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
        watch: false,
        auto_migrate_when_idle: false,
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
            repair_override: false,
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
            failure_reason_code: None,
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
    status(&paths, false, false).await.expect("status");
}

#[tokio::test]
async fn merge_integrate_squash_merges_and_cleans_successful_task() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("tasker.db");
    let pool = tasker_db::connect(&db_path).await.expect("connect");
    tasker_db::run_migrations(&pool).await.expect("migrate");
    let (repo, worktree) = seed_integrating_local_task(&pool, temp.path(), true, false).await;

    let result = integrate_local_worktree(
        &pool,
        "TASK-1",
        &tasker_db::Actor::operator("tester"),
        temp.path(),
    )
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
            "SELECT COUNT(*) FROM integration_outcomes WHERE outcome_kind = 'success' AND reason_code = 'success'",
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

    let result = integrate_local_worktree(
        &pool,
        "TASK-1",
        &tasker_db::Actor::operator("tester"),
        temp.path(),
    )
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
            "SELECT COUNT(*) FROM integration_outcomes WHERE outcome_kind = 'no_changes' AND reason_code = 'no_changes'",
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

    let result = integrate_local_worktree(
        &pool,
        "TASK-1",
        &tasker_db::Actor::operator("tester"),
        temp.path(),
    )
    .await
    .expect("integrate");

    assert!(result.summary.contains("operational Delivery Failure"));
    let detail = tasker_db::get_task_detail(&pool, "TASK-1")
        .await
        .expect("task")
        .expect("task");
    assert_eq!(detail.task.state, "integrating");
    let outcomes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM integration_outcomes WHERE outcome_kind = 'operational_failure' AND reason_code = 'dirty_managed_source_repository'",
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

    let result = integrate_local_worktree(
        &pool,
        "TASK-1",
        &tasker_db::Actor::operator("tester"),
        temp.path(),
    )
    .await
    .expect("integrate");

    assert!(result.summary.contains("work-change Delivery Failure"));
    let detail = tasker_db::get_task_detail(&pool, "TASK-1")
        .await
        .expect("task")
        .expect("task");
    assert_eq!(detail.task.state, "rework");
    let outcomes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM integration_outcomes WHERE outcome_kind = 'work_change_failure' AND reason_code = 'uncommitted_local_worktree'",
        )
        .fetch_one(&pool)
        .await
        .expect("outcome count");
    assert_eq!(outcomes, 1);
}

#[tokio::test]
async fn merge_retry_retries_retryable_operational_failure_without_new_agent_run() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = TaskerPaths::resolve(temp.path(), PathOverrides::default());
    init(&paths, false).await.expect("init");
    let pool = open_pool(&paths, false).await.expect("pool");
    seed_integrating_local_task(&pool, temp.path(), true, false).await;
    tasker_db::record_integration_outcome(
        &pool,
        &tasker_db::RecordIntegrationOutcomeInput {
            task_identifier: "TASK-1".to_string(),
            agent_run_id: None,
            outcome_kind: "operational_failure".to_string(),
            reason_code: "unknown_operational_failure".to_string(),
            final_commit: None,
            pre_merge_head: None,
            message: Some("fixed local operational issue".to_string()),
            retryable: true,
            retry_attempt: Some(1),
            retry_delay_seconds: Some(30),
        },
        &tasker_db::Actor::operator("tester"),
    )
    .await
    .expect("record operational failure");
    let runs_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs")
        .fetch_one(&pool)
        .await
        .expect("run count before");

    merge(
        &paths,
        false,
        MergeCommand::Retry {
            identifier: "TASK-1".to_string(),
            force: false,
            actor: "tester".to_string(),
        },
    )
    .await
    .expect("retry integration");

    let runs_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs")
        .fetch_one(&pool)
        .await
        .expect("run count after");
    assert_eq!(runs_before, runs_after);
    let detail = tasker_db::get_task_detail(&pool, "TASK-1")
        .await
        .expect("task")
        .expect("task");
    assert_eq!(detail.task.state, "done");
    let successes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM integration_outcomes WHERE outcome_kind = 'success'",
    )
    .fetch_one(&pool)
    .await
    .expect("success count");
    assert_eq!(successes, 1);
}

#[tokio::test]
async fn merge_retry_refuses_non_integrating_and_work_change_by_default() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = TaskerPaths::resolve(temp.path(), PathOverrides::default());
    init(&paths, false).await.expect("init");
    let pool = open_pool(&paths, false).await.expect("pool");
    seed_integrating_local_task(&pool, temp.path(), true, false).await;
    tasker_db::transition_task_state(
        &pool,
        "TASK-1",
        &tasker_db::TransitionTaskState {
            to_state: "rework".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &tasker_db::Actor::operator("tester"),
    )
    .await
    .expect("move to rework");

    let non_integrating = merge(
        &paths,
        false,
        MergeCommand::Retry {
            identifier: "TASK-1".to_string(),
            force: false,
            actor: "tester".to_string(),
        },
    )
    .await
    .expect_err("non-Integrating Task refused");
    assert!(non_integrating
        .to_string()
        .contains("Task State integrating"));

    let temp = tempfile::tempdir().expect("tempdir");
    let paths = TaskerPaths::resolve(temp.path(), PathOverrides::default());
    init(&paths, false).await.expect("init");
    let pool = open_pool(&paths, false).await.expect("pool");
    seed_integrating_local_task(&pool, temp.path(), true, false).await;
    tasker_db::record_integration_outcome(
        &pool,
        &tasker_db::RecordIntegrationOutcomeInput {
            task_identifier: "TASK-1".to_string(),
            agent_run_id: None,
            outcome_kind: "work_change_failure".to_string(),
            reason_code: "unknown_work_change_failure".to_string(),
            final_commit: None,
            pre_merge_head: None,
            message: Some("requires Task work changes".to_string()),
            retryable: false,
            retry_attempt: None,
            retry_delay_seconds: None,
        },
        &tasker_db::Actor::operator("tester"),
    )
    .await
    .expect("record work-change failure");

    let work_change = merge(
        &paths,
        false,
        MergeCommand::Retry {
            identifier: "TASK-1".to_string(),
            force: false,
            actor: "tester".to_string(),
        },
    )
    .await
    .expect_err("work-change failure refused");
    assert!(work_change.to_string().contains("work_change_failure"));
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

    let result = integrate_local_worktree(
        &pool,
        "TASK-1",
        &tasker_db::Actor::operator("tester"),
        temp.path(),
    )
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

    let result = integrate_local_worktree(
        &pool,
        "TASK-1",
        &tasker_db::Actor::operator("tester"),
        temp.path(),
    )
    .await
    .expect("integrate");

    assert!(result.summary.contains("work-change Delivery Failure"));
    assert!(result.summary.contains("Validated Base Commit"));
    let detail = tasker_db::get_task_detail(&pool, "TASK-1")
        .await
        .expect("task")
        .expect("task");
    assert_eq!(detail.task.state, "rework");
    let reason_code: String = sqlx::query_scalar(
        "SELECT reason_code FROM integration_outcomes ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("reason code");
    assert_eq!(reason_code, "stale_validated_base_commit");
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

    let result = integrate_local_worktree(
        &pool,
        "TASK-1",
        &tasker_db::Actor::operator("tester"),
        temp.path(),
    )
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
    let reason_code: String = sqlx::query_scalar(
        "SELECT reason_code FROM integration_outcomes ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("reason code");
    assert_eq!(reason_code, "task_branch_missing_main");
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

    let result = integrate_local_worktree(
        &pool,
        "TASK-1",
        &tasker_db::Actor::operator("tester"),
        temp.path(),
    )
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

#[tokio::test]
async fn worker_integrating_transition_rejects_dirty_local_worktree_without_state_change() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = TaskerPaths::resolve(temp.path().join("home"), PathOverrides::default());
    let (pool, worktree, run_id) =
        seed_in_progress_local_task(&paths, temp.path(), true, true).await;

    let error = task(
        &paths,
        false,
        TaskCommand::Transition {
            identifier: "TASK-1".to_string(),
            to: "integrating".to_string(),
            actor_kind: "worker_agent".to_string(),
            actor: "worker".to_string(),
            agent_run_id: Some(run_id),
            repair_override: false,
        },
    )
    .await
    .expect_err("dirty Local Worktree should be rejected before Integrating");
    let message = error.to_string();
    assert!(message.contains("Local Worktree pre-Integrating check failed"));
    assert!(message.contains(&worktree.display().to_string()));
    assert!(message.contains("tasker/TASK-1"));
    assert!(message.contains("git status summary"));
    assert!(message.contains("scratch.txt"));
    assert!(!message.contains("dirty contents"));
    assert!(message.contains("Commit intended changes on the Task Branch"));

    let detail = tasker_db::get_task_detail(&pool, "TASK-1")
        .await
        .expect("get task")
        .expect("task");
    assert_eq!(detail.task.state, "in_progress");
    let outcomes: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM integration_outcomes")
        .fetch_one(&pool)
        .await
        .expect("outcomes");
    assert_eq!(outcomes.0, 0);
}

#[tokio::test]
async fn worker_integrating_transition_allows_clean_local_worktree() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = TaskerPaths::resolve(temp.path().join("home"), PathOverrides::default());
    let (pool, _worktree, run_id) =
        seed_in_progress_local_task(&paths, temp.path(), true, false).await;

    task(
        &paths,
        false,
        TaskCommand::Transition {
            identifier: "TASK-1".to_string(),
            to: "integrating".to_string(),
            actor_kind: "worker_agent".to_string(),
            actor: "worker".to_string(),
            agent_run_id: Some(run_id),
            repair_override: false,
        },
    )
    .await
    .expect("clean Local Worktree should allow Integrating");

    let detail = tasker_db::get_task_detail(&pool, "TASK-1")
        .await
        .expect("get task")
        .expect("task");
    assert_eq!(detail.task.state, "integrating");
}

#[tokio::test]
async fn operator_integrating_transition_allows_dirty_local_worktree_for_repair_flexibility() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = TaskerPaths::resolve(temp.path().join("home"), PathOverrides::default());
    let (pool, _worktree, _run_id) =
        seed_in_progress_local_task(&paths, temp.path(), true, true).await;

    task(
        &paths,
        false,
        TaskCommand::Transition {
            identifier: "TASK-1".to_string(),
            to: "integrating".to_string(),
            actor_kind: "operator".to_string(),
            actor: "operator".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
    )
    .await
    .expect("operator repair transition should keep flexibility");

    let detail = tasker_db::get_task_detail(&pool, "TASK-1")
        .await
        .expect("get task")
        .expect("task");
    assert_eq!(detail.task.state, "integrating");
}

#[tokio::test]
async fn worker_integrating_transition_rejects_missing_local_worktree_links_with_guidance() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = TaskerPaths::resolve(temp.path().join("home"), PathOverrides::default());
    let (pool, _worktree, run_id) =
        seed_in_progress_local_task(&paths, temp.path(), false, false).await;

    let error = task(
        &paths,
        false,
        TaskCommand::Transition {
            identifier: "TASK-1".to_string(),
            to: "integrating".to_string(),
            actor_kind: "worker_agent".to_string(),
            actor: "worker".to_string(),
            agent_run_id: Some(run_id),
            repair_override: false,
        },
    )
    .await
    .expect_err("missing Local Worktree links should be rejected");
    let message = error.to_string();
    assert!(message.contains("missing Local Worktree Task Link"));
    assert!(message.contains("missing Task Branch Task Link"));
    assert!(message.contains("Commit intended changes on the Task Branch"));

    let detail = tasker_db::get_task_detail(&pool, "TASK-1")
        .await
        .expect("get task")
        .expect("task");
    assert_eq!(detail.task.state, "in_progress");
}

async fn seed_in_progress_local_task(
    paths: &TaskerPaths,
    root: &Path,
    with_links: bool,
    dirty: bool,
) -> (sqlx::SqlitePool, PathBuf, String) {
    init(paths, false).await.expect("init");
    let pool = open_pool(paths, false).await.expect("pool");
    let repo = root.join("repo");
    let worktrees = root.join("worktrees");
    let worktree = worktrees.join("TASK-1");
    init_git_repo(&repo);
    tasker_db::create_task_queue(
        &pool,
        &tasker_db::CreateTaskQueue {
            key: "TASK".to_string(),
            name: "Tasker".to_string(),
            managed_source_repository: repo.display().to_string(),
            main_branch: "main".to_string(),
            worktree_root: worktrees.display().to_string(),
            branch_template: "tasker/{task_identifier}".to_string(),
            done_worktree_retention: false,
            queue_concurrency_limit: None,
        },
        &tasker_db::Actor::operator("tester"),
    )
    .await
    .expect("queue");
    tasker_db::create_task(
        &pool,
        &tasker_db::CreateTask {
            queue_key: "TASK".to_string(),
            title: "Preflight me".to_string(),
            brief: "Brief".to_string(),
            priority: "normal".to_string(),
            state: "ready".to_string(),
            review_required: false,
            acceptance_criteria: vec!["accepted".to_string()],
            validation_items: vec!["validated".to_string()],
            tags: vec![],
            conflict_hints: vec![],
            blocking_task_identifiers: vec![],
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
    fs::write(worktree.join("feature.txt"), "feature\n").expect("feature");
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "add feature"]);
    let actor = tasker_db::Actor::operator("tester");
    if with_links {
        tasker_db::upsert_task_link(
            &pool,
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
            &pool,
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
    }
    tasker_db::update_acceptance_criterion_status(
        &pool,
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
        &pool,
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
    let worker = tasker_db::Actor {
        kind: "worker_agent".to_string(),
        id: "worker".to_string(),
        display_name: "worker".to_string(),
    };
    let claimed = tasker_db::claim_next(
        &pool,
        &tasker_db::ClaimNextInput {
            queue_key: "TASK".to_string(),
            worker_id: "worker-1".to_string(),
            launcher_kind: "pi".to_string(),
            lease_seconds: 300,
        },
        &worker,
    )
    .await
    .expect("claim")
    .expect("claimed");
    if dirty {
        fs::write(worktree.join("scratch.txt"), "dirty contents\n").expect("scratch");
    }
    (pool, worktree, claimed.run.id)
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
            blocking_task_identifiers: vec![],
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
            repair_override: false,
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
            repair_override: false,
        },
        &actor,
    )
    .await
    .expect("integrating");
    (repo, worktree)
}

async fn forget_applied_migrations(pool: &sqlx::SqlitePool) {
    sqlx::query("DELETE FROM _sqlx_migrations")
        .execute(pool)
        .await
        .expect("forget migrations");
}

async fn create_empty_migrations_table(pool: &sqlx::SqlitePool) {
    sqlx::query(
        r#"
            CREATE TABLE _sqlx_migrations (
                version BIGINT PRIMARY KEY,
                description TEXT NOT NULL,
                installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                success BOOLEAN NOT NULL,
                checksum BLOB NOT NULL,
                execution_time BIGINT NOT NULL
            )
            "#,
    )
    .execute(pool)
    .await
    .expect("create migrations table");
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

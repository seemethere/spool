use std::fs;

use clap::{CommandFactory, Parser};

use super::*;

#[test]
fn cli_definition_is_valid() {
    Cli::command().debug_assert();
}

#[test]
fn task_create_parses_preferred_from_file_shape() {
    let cli = Cli::try_parse_from([
        "tasker",
        "task",
        "create",
        "--queue",
        "TASK",
        "--from-file",
        "task.md",
    ])
    .expect("parse preferred file-backed create command");

    match cli.command.expect("command") {
        Command::Task {
            command:
                TaskCommand::Create {
                    bootstrap,
                    queue,
                    from_file,
                    file,
                    ..
                },
        } => {
            assert!(!bootstrap);
            assert_eq!(queue, "TASK");
            assert_eq!(
                from_file.expect("from file"),
                std::path::PathBuf::from("task.md")
            );
            assert!(file.is_none());
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn delegate_parses_create_and_refine_shapes() {
    let create = Cli::try_parse_from(["tasker", "delegate", "--queue", "TASK"])
        .expect("parse delegate create command");
    match create.command.expect("command") {
        Command::Delegate { queue, refine, .. } => {
            assert_eq!(queue.as_deref(), Some("TASK"));
            assert!(refine.is_none());
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let refine = Cli::try_parse_from(["tasker", "delegate", "--refine", "TASK-1"])
        .expect("parse delegate refine command");
    match refine.command.expect("command") {
        Command::Delegate { queue, refine, .. } => {
            assert!(queue.is_none());
            assert_eq!(refine.as_deref(), Some("TASK-1"));
        }
        other => panic!("unexpected command: {other:?}"),
    }
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
                from_file: None,
                file: Some(repo.join("task.md")),
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
                from_file: None,
                file: Some(repo.join("task.md")),
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
            from_file: None,
            file: Some(temp.path().join("task.md")),
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
        telemetry: tasker_config::TelemetryConfig::default(),
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
            from_file: None,
            file: Some(task_file),
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

    merge(
        &paths,
        false,
        MergeCommand::Lock {
            command: MergeLockCommand::Acquire {
                queue: "TASK".to_string(),
                operation: "manual_integration".to_string(),
                task: Some("TASK-1".to_string()),
            },
        },
    )
    .await
    .expect("acquire operation lock");
    assert!(
        tasker_runner::repo_lock::active_lock(&paths.data_dir, "TASK")
            .expect("active lock")
            .is_some()
    );
    merge(
        &paths,
        false,
        MergeCommand::Lock {
            command: MergeLockCommand::Status {
                queue: "TASK".to_string(),
            },
        },
    )
    .await
    .expect("show operation lock");
    merge(
        &paths,
        false,
        MergeCommand::Lock {
            command: MergeLockCommand::Release {
                queue: "TASK".to_string(),
            },
        },
    )
    .await
    .expect("release operation lock");
    assert!(
        tasker_runner::repo_lock::active_lock(&paths.data_dir, "TASK")
            .expect("active lock")
            .is_none()
    );

    task(
        &paths,
        false,
        TaskCommand::Create {
            bootstrap: true,
            queue: "TASK".to_string(),
            from_file: None,
            file: Some(temp.path().join("task.md")),
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
async fn task_review_decision_command_records_rework_feedback() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (paths, pool) = seed_cli_human_review_task(temp.path(), "Review me").await;

    task(
        &paths,
        false,
        TaskCommand::ReviewDecision {
            identifier: "TASK-1".to_string(),
            decision: "rework".to_string(),
            feedback: Some("Address the review feedback.".to_string()),
            feedback_file: None,
            actor_kind: "review_agent".to_string(),
            actor: "reviewer".to_string(),
        },
    )
    .await
    .expect("record Review Decision");

    let detail = tasker_db::get_task_detail(&pool, "TASK-1")
        .await
        .expect("load task")
        .expect("task exists");
    assert_eq!(detail.task.state, "rework");
    assert!(detail
        .workpad_note
        .expect("Workpad Note")
        .body
        .contains("Address the review feedback"));
}

#[tokio::test]
async fn task_review_decision_command_records_approve_to_integrating() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (paths, pool) = seed_cli_human_review_task(temp.path(), "Approve me").await;

    task(
        &paths,
        false,
        TaskCommand::ReviewDecision {
            identifier: "TASK-1".to_string(),
            decision: "approve".to_string(),
            feedback: None,
            feedback_file: None,
            actor_kind: "review_agent".to_string(),
            actor: "reviewer".to_string(),
        },
    )
    .await
    .expect("record approve Review Decision");

    let detail = tasker_db::get_task_detail(&pool, "TASK-1")
        .await
        .expect("load task")
        .expect("task exists");
    assert_eq!(detail.task.state, "integrating");
    let events = tasker_db::list_task_audit_events(&pool, "TASK-1")
        .await
        .expect("audit events");
    assert!(events.iter().any(|event| {
        event.event_type == "task.review_decision_recorded"
            && event.actor_kind == "review_agent"
            && event.payload_json.contains("\"decision\":\"approve\"")
    }));
}

async fn seed_cli_human_review_task(temp: &Path, title: &str) -> (TaskerPaths, sqlx::SqlitePool) {
    let paths = TaskerPaths::resolve(temp, PathOverrides::default());
    init(&paths, false).await.expect("init");
    let repo = temp.join("repo");
    init_git_repo(&repo);
    queue(
        &paths,
        false,
        QueueCommand::Create {
            key: "TASK".to_string(),
            name: "Tasker".to_string(),
            managed_source_repository: repo,
            main_branch: "main".to_string(),
            worktree_root: temp.join("worktrees"),
            branch_template: "tasker/{task_identifier}".to_string(),
            done_worktree_retention: false,
            queue_concurrency_limit: None,
            actor: "tester".to_string(),
        },
    )
    .await
    .expect("create queue");
    let task_file = temp.join("review-task.md");
    fs::write(
        &task_file,
        format!(
            r#"---
title: {title}
acceptance_criteria:
  - It works
validation_items:
  - Tests pass
---
Implement reviewable work.
"#
        ),
    )
    .expect("write task file");
    task(
        &paths,
        false,
        TaskCommand::Create {
            bootstrap: false,
            queue: "TASK".to_string(),
            from_file: Some(task_file),
            file: None,
            actor: "tester".to_string(),
        },
    )
    .await
    .expect("create task");
    let pool = open_pool(&paths, false).await.expect("pool");
    let actor = tasker_db::Actor::operator("tester");
    tasker_db::transition_task_state(
        &pool,
        "TASK-1",
        &tasker_db::TransitionTaskState {
            to_state: "in_progress".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &actor,
    )
    .await
    .expect("start");
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
    tasker_db::transition_task_state(
        &pool,
        "TASK-1",
        &tasker_db::TransitionTaskState {
            to_state: "human_review".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &actor,
    )
    .await
    .expect("human review");

    (paths, pool)
}

#[cfg(unix)]
#[tokio::test]
async fn review_command_launches_pi_backed_interactive_review_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = TaskerPaths::resolve(temp.path(), PathOverrides::default());
    init(&paths, false).await.expect("init");
    let repo = temp.path().join("repo");
    init_git_repo(&repo);
    let prompt_dir = repo.join(".tasker/prompts");
    fs::create_dir_all(&prompt_dir).expect("prompt dir");
    fs::write(
        prompt_dir.join("review.md"),
        "Custom repo Review Agent prompt.",
    )
    .expect("write prompt override");
    queue(
        &paths,
        false,
        QueueCommand::Create {
            key: "TASK".to_string(),
            name: "Tasker".to_string(),
            managed_source_repository: repo.clone(),
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
    let task_file = temp.path().join("review-task.md");
    fs::write(
        &task_file,
        r#"---
title: Review launch
acceptance_criteria:
  - It works
validation_items:
  - Tests pass
---
Implement reviewable work.
"#,
    )
    .expect("write task file");
    task(
        &paths,
        false,
        TaskCommand::Create {
            bootstrap: false,
            queue: "TASK".to_string(),
            from_file: Some(task_file),
            file: None,
            actor: "tester".to_string(),
        },
    )
    .await
    .expect("create task");
    let pool = open_pool(&paths, false).await.expect("pool");
    let actor = tasker_db::Actor::operator("tester");
    tasker_db::transition_task_state(
        &pool,
        "TASK-1",
        &tasker_db::TransitionTaskState {
            to_state: "in_progress".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &actor,
    )
    .await
    .expect("start");
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
    tasker_db::transition_task_state(
        &pool,
        "TASK-1",
        &tasker_db::TransitionTaskState {
            to_state: "human_review".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &actor,
    )
    .await
    .expect("human review");

    let capture = temp.path().join("review-prompt.jsonl");
    let pi_bin = temp.path().join("fake-pi");
    fs::write(
        &pi_bin,
        format!(
            r#"#!/bin/sh
cat > "{capture}"
printf '%s\n' '{{"type":"extension_ui_request","method":"select"}}'
printf '%s\n' '{{"type":"agent_end"}}'
printf '%s\n' "$TASKER_ACTOR_KIND:$TASKER_ACTOR_ID" >> "{capture}"
"#,
            capture = capture.display()
        ),
    )
    .expect("write fake pi");
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(&pi_bin).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&pi_bin, permissions).expect("chmod");

    review(
        &paths,
        false,
        ReviewOptions {
            identifier: "TASK-1".to_string(),
            actor: "reviewer".to_string(),
            api_url: Some("http://tasker.test".to_string()),
            pi_bin: pi_bin.display().to_string(),
            pi_extension: None,
        },
    )
    .await
    .expect("review session");

    let captured = fs::read_to_string(capture).expect("capture");
    assert!(captured.contains("Custom repo Review Agent prompt."));
    assert!(captured.contains("Review Packet"));
    assert!(captured.contains("Question UI is allowed"));
    assert!(captured.contains("tasker_record_review_decision"));
    assert!(captured.contains("review_agent:reviewer"));
}

#[cfg(unix)]
#[tokio::test]
async fn delegate_command_launches_create_and_refine_interactive_sessions() {
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
            managed_source_repository: repo.clone(),
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
    let task_file = temp.path().join("refine-task.md");
    fs::write(
        &task_file,
        r#"---
title: Refine me
state: backlog
---
Needs a better contract.
"#,
    )
    .expect("write task file");
    task(
        &paths,
        false,
        TaskCommand::Create {
            bootstrap: false,
            queue: "TASK".to_string(),
            from_file: Some(task_file),
            file: None,
            actor: "tester".to_string(),
        },
    )
    .await
    .expect("create backlog task");

    let capture = temp.path().join("delegate-prompt.jsonl");
    let pi_bin = temp.path().join("fake-pi");
    fs::write(
        &pi_bin,
        format!(
            r#"#!/bin/sh
cat >> "{capture}"
printf '%s\n' '{{"type":"extension_ui_request","method":"input"}}'
printf '%s\n' '{{"type":"agent_end"}}'
printf '%s\n' "$TASKER_ACTOR_KIND:$TASKER_ACTOR_ID" >> "{capture}"
"#,
            capture = capture.display()
        ),
    )
    .expect("write fake pi");
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(&pi_bin).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&pi_bin, permissions).expect("chmod");

    delegate(
        &paths,
        false,
        DelegateOptions {
            queue: Some("TASK".to_string()),
            refine: None,
            actor: "delegator".to_string(),
            api_url: Some("http://tasker.test".to_string()),
            pi_bin: pi_bin.display().to_string(),
            pi_extension: None,
        },
    )
    .await
    .expect("delegate create session");
    delegate(
        &paths,
        false,
        DelegateOptions {
            queue: None,
            refine: Some("TASK-1".to_string()),
            actor: "delegator".to_string(),
            api_url: Some("http://tasker.test".to_string()),
            pi_bin: pi_bin.display().to_string(),
            pi_extension: None,
        },
    )
    .await
    .expect("delegate refine session");

    let captured = fs::read_to_string(capture).expect("capture");
    assert!(captured.contains("Task Queue Key: TASK"));
    assert!(captured.contains("tasker_create_delegated_root_task"));
    assert!(captured.contains("Refinement target: TASK-1"));
    assert!(captured.contains("Existing Backlog Task context for refinement"));
    assert!(captured.contains("tasker_refine_backlog_task"));
    assert!(captured.contains("delegating_agent:delegator"));
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

#[tokio::test]
async fn bootstrap_create_persists_canonical_priority_for_medium_alias() {
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
title: Normalize bootstrap priority
priority: medium
acceptance_criteria:
  - Priority alias is accepted
validation_items:
  - Stored priority is canonical
---
Implement priority alias normalization.
"#,
    )
    .expect("write task file");

    task(
        &paths,
        false,
        TaskCommand::Create {
            bootstrap: true,
            queue: "TASK".to_string(),
            from_file: None,
            file: Some(task_file),
            actor: "tester".to_string(),
        },
    )
    .await
    .expect("create task");

    let pool = tasker_db::connect(&paths.db_path).await.expect("connect");
    let detail = tasker_db::get_task_detail(&pool, "TASK-1")
        .await
        .expect("get task")
        .expect("task exists");
    assert_eq!(detail.task.priority, "normal");
}

#[tokio::test]
async fn file_backed_create_accepts_from_file_without_bootstrap_flag() {
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
        "---\ntitle: File-backed create\nacceptance_criteria:\n  - Preferred flag works\nvalidation_items:\n  - Task is persisted\n---\nBrief\n",
    )
    .expect("write task file");

    task(
        &paths,
        false,
        TaskCommand::Create {
            bootstrap: false,
            queue: "TASK".to_string(),
            from_file: Some(task_file),
            file: None,
            actor: "tester".to_string(),
        },
    )
    .await
    .expect("create task from file");

    let pool = tasker_db::connect(&paths.db_path).await.expect("connect");
    let detail = tasker_db::get_task_detail(&pool, "TASK-1")
        .await
        .expect("get task")
        .expect("task exists");
    assert_eq!(detail.task.title, "File-backed create");
}

#[tokio::test]
async fn file_compatibility_flag_still_requires_bootstrap() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = TaskerPaths::resolve(temp.path(), PathOverrides::default());
    init(&paths, false).await.expect("init");
    let task_file = temp.path().join("task.md");
    fs::write(
        &task_file,
        "---\ntitle: Legacy file\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Error is clear\n---\nBrief\n",
    )
    .expect("write task file");

    let error = task(
        &paths,
        false,
        TaskCommand::Create {
            bootstrap: false,
            queue: "TASK".to_string(),
            from_file: None,
            file: Some(task_file),
            actor: "tester".to_string(),
        },
    )
    .await
    .expect_err("--file without --bootstrap should fail");

    assert!(error.to_string().contains("prefer task create --from-file"));
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

#[tokio::test]
async fn bootstrap_lint_succeeds_for_valid_task_file_without_database() {
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = TaskerPaths::resolve(temp.path().join("missing-config"), PathOverrides::default());
    let task_file = temp.path().join("task.md");
    fs::write(
        &task_file,
        "---\ntitle: Test lint\npriority: high\nstate: backlog\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Tests pass\n---\nBrief\n",
    )
    .expect("write task file");

    task(
        &paths,
        false,
        TaskCommand::Lint {
            file: task_file.clone(),
        },
    )
    .await
    .expect("lint valid task file");

    let parsed = bootstrap::lint_bootstrap_task_file(&task_file).expect("parse lint output source");
    assert_eq!(parsed.title, "Test lint");
    assert_eq!(parsed.priority, "high");
    assert_eq!(parsed.state, "backlog");
}

#[test]
fn bootstrap_lint_fails_for_invalid_priority_and_missing_required_fields() {
    let temp = tempfile::tempdir().expect("tempdir");
    let invalid_priority = temp.path().join("invalid-priority.md");
    fs::write(
        &invalid_priority,
        "---\ntitle: Test\npriority: maybe\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Tests pass\n---\nBrief\n",
    )
    .expect("write invalid priority file");
    let error =
        bootstrap::lint_bootstrap_task_file(&invalid_priority).expect_err("invalid priority fails");
    let message = error.to_string();
    assert!(message.contains("invalid priority \"maybe\""));
    assert!(message.contains("expected one of: urgent, high, normal, low"));

    let missing_required = temp.path().join("missing-required.md");
    fs::write(
        &missing_required,
        "---\npriority: normal\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Tests pass\n---\nBrief\n",
    )
    .expect("write missing required file");
    let error =
        bootstrap::lint_bootstrap_task_file(&missing_required).expect_err("missing title fails");
    assert!(error
        .to_string()
        .contains("failed to parse YAML front matter"));
    assert!(error.to_string().contains("missing field `title`"));
}

#[tokio::test]
async fn bootstrap_lint_does_not_create_task_or_audit_events() {
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
        "---\ntitle: Test lint\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Tests pass\n---\nBrief\n",
    )
    .expect("write task file");

    task(&paths, false, TaskCommand::Lint { file: task_file })
        .await
        .expect("lint task file");

    let pool = open_pool(&paths, false).await.expect("open pool");
    let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(&pool)
        .await
        .expect("count tasks");
    let task_audit_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE subject_type = 'task'")
            .fetch_one(&pool)
            .await
            .expect("count task audit events");

    assert_eq!(task_count, 0);
    assert_eq!(task_audit_count, 0);
}

#[test]
fn bootstrap_parser_normalizes_medium_priority_alias() {
    let parsed = bootstrap::parse_bootstrap_task_with_warnings(
        "TASK",
        "inline",
        "---\ntitle: Test\npriority: medium\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Tests pass\n---\nBrief\n",
    )
    .expect("parse");

    assert_eq!(parsed.task.priority, "normal");
    assert_eq!(
        parsed.warnings,
        vec!["normalized priority alias \"medium\" to canonical \"normal\"".to_string()]
    );
}

#[test]
fn bootstrap_parser_requires_front_matter() {
    let error = bootstrap::parse_bootstrap_task("TASK", "inline", "title: Missing delimiters")
        .expect_err("missing front matter fails");

    assert!(error.to_string().contains("must start"));
}

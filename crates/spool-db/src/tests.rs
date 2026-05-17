#![allow(unused_imports)]

use super::*;
use anyhow::Context;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};
use std::{
    fs,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::sleep;

use std::sync::atomic::{AtomicUsize, Ordering};

use sqlx::Row;

#[tokio::test]
async fn migrations_are_idempotent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("spool.db");
    let pool = connect(&db_path).await.expect("connect");

    run_migrations(&pool).await.expect("first migrate");
    run_migrations(&pool).await.expect("second migrate");

    let row = sqlx::query("select value from spool_metadata where key = 'schema_version'")
        .fetch_one(&pool)
        .await
        .expect("metadata row");
    let value: String = row.get("value");

    assert_eq!(value, "1");

    let legacy_table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'tasker_metadata'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy metadata table lookup");
    assert_eq!(legacy_table_count, 0);
}

#[tokio::test]
async fn spool_rename_migration_updates_metadata_and_metric_schema() {
    let (_temp, pool) = migrated_pool().await;

    let metadata_table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'spool_metadata'",
    )
    .fetch_one(&pool)
    .await
    .expect("spool metadata table lookup");
    assert_eq!(metadata_table_count, 1);

    let metric_columns: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('agent_run_metrics') WHERE name LIKE 'repeated_%_context_fetch_count' ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .expect("metric columns");
    assert_eq!(metric_columns, vec!["repeated_spool_context_fetch_count"]);
}

#[tokio::test]
async fn agent_run_metrics_derivation_version_migration_adds_legacy_default() {
    let (_temp, pool) = migrated_pool().await;

    let columns: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT name, dflt_value = '0' AS has_legacy_default
        FROM pragma_table_info('agent_run_metrics')
        WHERE name = 'derivation_version'
        "#,
    )
    .fetch_all(&pool)
    .await
    .expect("inspect metrics columns");

    assert_eq!(columns, vec![("derivation_version".to_string(), 1)]);
}

#[tokio::test]
async fn sqlite_write_retry_retries_busy_errors_with_bounded_backoff() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("spool.db");
    let options = || {
        SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .busy_timeout(Duration::from_millis(1))
    };
    let lock_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options())
        .await
        .expect("connect lock pool");
    let write_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options())
        .await
        .expect("connect write pool");
    sqlx::query("CREATE TABLE retry_probe (id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
        .execute(&lock_pool)
        .await
        .expect("create probe table");

    let mut tx = lock_pool.begin().await.expect("begin lock tx");
    sqlx::query("INSERT INTO retry_probe (value) VALUES ('lock holder')")
        .execute(&mut *tx)
        .await
        .expect("hold write lock");
    let attempts = Arc::new(AtomicUsize::new(0));
    let write_attempts = Arc::clone(&attempts);
    let write = with_sqlite_write_retry(|| {
        write_attempts.fetch_add(1, Ordering::SeqCst);
        async {
            sqlx::query("INSERT INTO retry_probe (value) VALUES ('retried')")
                .execute(&write_pool)
                .await
                .context("failed to write retry probe")?;
            Ok(())
        }
    });
    let release = async move {
        sleep(Duration::from_millis(15)).await;
        tx.rollback().await.expect("release lock");
    };

    let (write_result, _) = tokio::join!(write, release);
    write_result.expect("write succeeds after retry");
    assert!(attempts.load(Ordering::SeqCst) > 1);
}

#[tokio::test]
async fn sqlite_write_retry_does_not_retry_non_transient_errors() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("spool.db");
    let pool = connect(&db_path).await.expect("connect");
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed_attempts = Arc::clone(&attempts);

    let error = with_sqlite_write_retry(|| {
        observed_attempts.fetch_add(1, Ordering::SeqCst);
        async {
            sqlx::query("INSERT INTO missing_retry_probe (value) VALUES ('permanent')")
                .execute(&pool)
                .await
                .context("failed to write missing retry probe")?;
            Ok(())
        }
    })
    .await
    .expect_err("permanent SQL errors are returned");

    assert!(error
        .to_string()
        .contains("failed to write missing retry probe"));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn local_api_token_is_created_once_and_authenticates() {
    let (_temp, pool) = migrated_pool().await;

    let token = ensure_local_api_token(&pool).await.expect("create token");
    let same_token = ensure_local_api_token(&pool).await.expect("reuse token");

    assert_eq!(token, same_token);
    assert!(authenticate_api_token(&pool, &token)
        .await
        .expect("valid token authenticates"));
    assert!(!authenticate_api_token(&pool, "not-the-token")
        .await
        .expect("invalid token rejected"));
}

#[tokio::test]
async fn creates_gets_and_lists_task_queues() {
    let (_temp, pool) = migrated_pool().await;
    let first = sample_queue("TASK", "Spool");
    let second = sample_queue("OPS", "Operations");

    let created = create_task_queue(&pool, &first, &Actor::operator("tester"))
        .await
        .expect("create queue");
    create_task_queue(&pool, &second, &Actor::operator("tester"))
        .await
        .expect("create second queue");

    assert_eq!(created.key, "TASK");
    assert_eq!(created.delivery_backend, "local_worktree");
    assert!(!created.done_worktree_retention);

    let loaded = get_task_queue(&pool, "TASK")
        .await
        .expect("get queue")
        .expect("queue exists");
    assert_eq!(loaded, created);

    let queues = list_task_queues(&pool).await.expect("list queues");
    assert_eq!(
        queues
            .iter()
            .map(|queue| queue.key.as_str())
            .collect::<Vec<_>>(),
        vec!["OPS", "TASK"]
    );
}

#[tokio::test]
async fn task_queue_key_must_not_contain_path_separators() {
    let (_temp, pool) = migrated_pool().await;
    let input = sample_queue("BAD/KEY", "Bad Queue");

    let error = create_task_queue(&pool, &input, &Actor::operator("tester"))
        .await
        .expect_err("path separator key fails");

    assert!(error.to_string().contains("path separators"));
}

#[tokio::test]
async fn duplicate_task_queue_key_is_rejected() {
    let (_temp, pool) = migrated_pool().await;
    let input = sample_queue("TASK", "Spool");

    create_task_queue(&pool, &input, &Actor::operator("tester"))
        .await
        .expect("first create");
    let error = create_task_queue(&pool, &input, &Actor::operator("tester"))
        .await
        .expect_err("duplicate key fails");

    assert!(error
        .to_string()
        .contains("failed to create Task Queue TASK"));
}

#[tokio::test]
async fn task_queue_creation_appends_audit_event() {
    let (_temp, pool) = migrated_pool().await;
    let input = sample_queue("TASK", "Spool");

    let queue = create_task_queue(&pool, &input, &Actor::operator("tester"))
        .await
        .expect("create queue");

    let events = list_audit_events(&pool).await.expect("audit events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "task_queue.created");
    assert_eq!(events[0].subject_id, queue.id);
    assert_eq!(events[0].actor_kind, "operator");
    assert!(events[0].payload_json.contains("\"key\":\"TASK\""));
}

#[tokio::test]
async fn updates_task_queue_concurrency_limit_and_audit_history() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");

    let updated = update_task_queue_concurrency_limit(
        &pool,
        "TASK",
        &UpdateQueueConcurrencyLimit {
            queue_concurrency_limit: Some(2),
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("set limit");
    assert_eq!(updated.queue_concurrency_limit, Some(2));

    let cleared = update_task_queue_concurrency_limit(
        &pool,
        "TASK",
        &UpdateQueueConcurrencyLimit {
            queue_concurrency_limit: None,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("clear limit");
    assert_eq!(cleared.queue_concurrency_limit, None);

    let events = list_task_queue_audit_events(&pool, "TASK")
        .await
        .expect("queue audit events");
    assert_eq!(events.len(), 3);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "task_queue.concurrency_limit_updated")
            .count(),
        2
    );
    assert!(events
        .iter()
        .any(|event| event.payload_json.contains("\"queue_concurrency_limit\":2")));
    assert!(events.iter().any(|event| event
        .payload_json
        .contains("\"previous_queue_concurrency_limit\":2")
        && event
            .payload_json
            .contains("\"queue_concurrency_limit\":null")));
}

#[tokio::test]
async fn queue_concurrency_limit_update_must_be_positive() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");

    let error = update_task_queue_concurrency_limit(
        &pool,
        "TASK",
        &UpdateQueueConcurrencyLimit {
            queue_concurrency_limit: Some(0),
        },
        &Actor::operator("tester"),
    )
    .await
    .expect_err("zero limit fails");

    assert!(error
        .to_string()
        .contains("Queue Concurrency Limit must be positive"));
}

#[tokio::test]
async fn creates_tasks_with_queue_prefixed_identifiers_and_details() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task_queue(
        &pool,
        &sample_queue("OPS", "Operations"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create ops queue");

    let first = create_task(
        &pool,
        &sample_task("TASK", "First"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create first task");
    let second = create_task(
        &pool,
        &sample_task("TASK", "Second"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create second task");
    let other_queue = create_task(
        &pool,
        &sample_task("OPS", "Other"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create other queue task");

    assert_eq!(first.task.identifier, "TASK-1");
    assert_eq!(second.task.identifier, "TASK-2");
    assert_eq!(other_queue.task.identifier, "OPS-1");
    assert_eq!(first.acceptance_criteria[0].description, "It works");
    assert_eq!(first.validation_items[0].description, "cargo test passes");
    assert_eq!(first.tags, vec!["backend", "dogfood"]);

    let loaded = get_task_detail(&pool, "TASK-1")
        .await
        .expect("load task")
        .expect("task exists");
    assert_eq!(loaded.task.title, "First");
    assert_eq!(loaded.task.task_queue_key, "TASK");
}

#[tokio::test]
async fn ready_task_requires_structured_requirements() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    let mut input = sample_task("TASK", "Invalid");
    input.acceptance_criteria.clear();

    let error = create_task(&pool, &input, &Actor::operator("tester"))
        .await
        .expect_err("ready task without criteria fails");

    assert!(error.to_string().contains("Ready Tasks require"));
}

#[tokio::test]
async fn bootstrap_task_creation_rejects_later_lifecycle_states() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    let mut input = sample_task("TASK", "Invalid State");
    input.state = "done".to_string();

    let error = create_task(&pool, &input, &Actor::operator("tester"))
        .await
        .expect_err("later lifecycle state fails");

    assert!(error.to_string().contains("only supports Backlog or Ready"));
}

#[tokio::test]
async fn delegation_task_draft_creates_valid_root_task_with_structured_fields() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");

    let draft = DelegationTaskDraft {
        queue_key: " TASK ".to_string(),
        title: " Add delegate helper ".to_string(),
        brief: " Create a deterministic helper. ".to_string(),
        priority: "High".to_string(),
        initial_state: "Ready".to_string(),
        review_required: true,
        tags: vec![
            "delegation".to_string(),
            "dogfood".to_string(),
            "delegation".to_string(),
        ],
        conflict_hints: vec!["crates/spool-db".to_string(), "crates/spool-db".to_string()],
        blocking_task_identifiers: vec![],
        acceptance_criteria: vec!["Draft validates".to_string()],
        validation_items: vec!["cargo test -p spool-db".to_string()],
    };

    let created = create_delegated_root_task(&pool, &draft, &delegating_actor())
        .await
        .expect("create delegated Root Task");

    assert_eq!(created.task.identifier, "TASK-1");
    assert_eq!(created.task.title, "Add delegate helper");
    assert_eq!(created.task.priority, "high");
    assert_eq!(created.task.state, "ready");
    assert!(created.task.review_required);
    assert_eq!(
        created.acceptance_criteria[0].description,
        "Draft validates"
    );
    assert_eq!(
        created.validation_items[0].description,
        "cargo test -p spool-db"
    );
    assert_eq!(created.tags, vec!["delegation", "dogfood"]);
    assert_eq!(created.conflict_hints[0].target, "crates/spool-db");
}

#[test]
fn delegation_task_draft_rejects_ready_without_structured_gates() {
    let mut draft = sample_delegation_draft("TASK", "Missing gates");
    draft.initial_state = "ready".to_string();
    draft.acceptance_criteria.clear();

    let error = validate_delegation_task_draft(&draft).expect_err("Ready without gates fails");

    let message = error.to_string();
    assert!(message.contains("Ready Task drafts require"));
    assert!(message.contains("acceptance_criteria"));
    assert!(message.contains("validation_items"));
}

#[test]
fn delegation_task_draft_rejects_unsupported_fields() {
    let error = serde_json::from_value::<DelegationTaskDraft>(serde_json::json!({
        "queue_key": "TASK",
        "title": "Unsupported field",
        "brief": "Brief",
        "estimate": "not a v1 Spool field"
    }))
    .expect_err("unsupported Delegating Agent fields fail");

    assert!(error.to_string().contains("unknown field"));
}

#[tokio::test]
async fn delegation_task_draft_records_blockers_conflict_hints_and_review_requirement() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Blocking"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create Blocking Task");

    let mut draft = sample_delegation_draft("TASK", "Blocked draft");
    draft.initial_state = "ready".to_string();
    draft.review_required = true;
    draft.conflict_hints = vec!["docs/DELEGATION_SESSION.md".to_string()];
    draft.blocking_task_identifiers = vec![" task-1 ".to_string()];

    let created = create_delegated_root_task(&pool, &draft, &delegating_actor())
        .await
        .expect("create blocked delegated Root Task");

    assert!(created.task.review_required);
    assert_eq!(created.blocking_tasks.len(), 1);
    assert_eq!(created.blocking_tasks[0].identifier, "TASK-1");
    assert_eq!(
        created.conflict_hints[0].target,
        "docs/DELEGATION_SESSION.md"
    );
}

#[tokio::test]
async fn delegation_task_draft_rejects_cross_queue_blockers() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task_queue(
        &pool,
        &sample_queue("OTHER", "Other"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create other queue");
    create_task(
        &pool,
        &sample_task("OTHER", "Other blocker"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create other Blocking Task");

    let mut draft = sample_delegation_draft("TASK", "Cross queue blocker");
    draft.blocking_task_identifiers = vec!["OTHER-1".to_string()];

    let error = create_delegated_root_task(&pool, &draft, &delegating_actor())
        .await
        .expect_err("cross-queue blocker fails");

    assert!(error.to_string().contains("same Task Queue"));
}

#[tokio::test]
async fn backlog_task_may_be_created_before_requirements_are_complete() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    let mut input = sample_task("TASK", "Backlog");
    input.state = "backlog".to_string();
    input.acceptance_criteria.clear();
    input.validation_items.clear();

    let created = create_task(&pool, &input, &Actor::operator("tester"))
        .await
        .expect("create backlog task");

    assert_eq!(created.task.identifier, "TASK-1");
    assert!(created.acceptance_criteria.is_empty());
}

#[tokio::test]
async fn refine_backlog_task_updates_contract_and_can_promote_ready() {
    let (_temp, pool) = migrated_pool().await;
    let actor = Actor::operator("tester");
    create_task_queue(&pool, &sample_queue("TASK", "Spool"), &actor)
        .await
        .expect("create queue");
    create_task(&pool, &sample_task("TASK", "Blocking"), &actor)
        .await
        .expect("create blocking task");
    let mut backlog = sample_task("TASK", "Rough Backlog Task");
    backlog.state = "backlog".to_string();
    backlog.acceptance_criteria.clear();
    backlog.validation_items.clear();
    backlog.tags = vec!["rough".to_string()];
    create_task(&pool, &backlog, &actor)
        .await
        .expect("create backlog task");

    let detail = refine_backlog_task(
        &pool,
        "TASK-2",
        &RefineBacklogTask {
            title: Some("Agent-ready Backlog Task".to_string()),
            brief: Some("# Task Brief\n\nClear enough for a Worker Agent.".to_string()),
            priority: Some("high".to_string()),
            target_state: Some("ready".to_string()),
            review_required: Some(true),
            acceptance_criteria: vec!["A deterministic helper refines the Task".to_string()],
            validation_items: vec!["Database tests cover refinement".to_string()],
            tags: Some(vec!["delegation".to_string(), "dogfood".to_string()]),
            conflict_hints: Some(vec!["crates/spool-db".to_string()]),
            blocking_task_identifiers: Some(vec![" task-1 ".to_string(), "TASK-1".to_string()]),
        },
        &delegating_actor(),
    )
    .await
    .expect("refine backlog task");

    assert_eq!(detail.task.title, "Agent-ready Backlog Task");
    assert_eq!(detail.task.state, "ready");
    assert_eq!(detail.task.priority, "high");
    assert!(detail.task.review_required);
    assert_eq!(detail.acceptance_criteria[0].status, "pending");
    assert_eq!(detail.validation_items[0].status, "pending");
    assert_eq!(detail.tags, vec!["delegation", "dogfood"]);
    assert_eq!(detail.conflict_hints[0].target, "crates/spool-db");
    assert_eq!(detail.blocking_tasks[0].identifier, "TASK-1");

    let events = list_task_audit_events(&pool, "TASK-2")
        .await
        .expect("audit events");
    assert!(events
        .iter()
        .any(|event| event.event_type == "task.refined"));
    assert!(events
        .iter()
        .any(|event| event.event_type == "task.state_transitioned"));
}

#[tokio::test]
async fn refine_backlog_task_preserves_or_resets_requirement_statuses() {
    let (_temp, pool) = migrated_pool().await;
    let actor = Actor::operator("tester");
    create_task_queue(&pool, &sample_queue("TASK", "Spool"), &actor)
        .await
        .expect("create queue");
    let mut backlog = sample_task("TASK", "Requirements");
    backlog.state = "backlog".to_string();
    backlog.acceptance_criteria = vec![
        "Keep criterion".to_string(),
        "Clarify criterion".to_string(),
    ];
    backlog.validation_items = vec![
        "Keep validation".to_string(),
        "Clarify validation".to_string(),
    ];
    create_task(&pool, &backlog, &actor)
        .await
        .expect("create backlog task");
    update_acceptance_criterion_status(
        &pool,
        "TASK-1",
        1,
        &UpdateRequirementStatus {
            status: "satisfied".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &actor,
    )
    .await
    .expect("satisfy criterion");
    update_acceptance_criterion_status(
        &pool,
        "TASK-1",
        2,
        &UpdateRequirementStatus {
            status: "satisfied".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &actor,
    )
    .await
    .expect("satisfy second criterion");
    update_validation_item_status(
        &pool,
        "TASK-1",
        1,
        &UpdateRequirementStatus {
            status: "passed".to_string(),
            waiver_reason: None,
            validated_base_commit: Some("abc123".to_string()),
        },
        &actor,
    )
    .await
    .expect("pass validation");

    let detail = refine_backlog_task(
        &pool,
        "TASK-1",
        &RefineBacklogTask {
            title: None,
            brief: None,
            priority: None,
            target_state: None,
            review_required: None,
            acceptance_criteria: vec![
                "Keep criterion".to_string(),
                "Clarified criterion".to_string(),
                "New criterion".to_string(),
            ],
            validation_items: vec![
                "Keep validation".to_string(),
                "Clarified validation".to_string(),
            ],
            tags: None,
            conflict_hints: None,
            blocking_task_identifiers: None,
        },
        &delegating_actor(),
    )
    .await
    .expect("refine requirements");

    assert_eq!(detail.acceptance_criteria[0].status, "satisfied");
    assert_eq!(detail.acceptance_criteria[1].status, "pending");
    assert_eq!(detail.acceptance_criteria[2].status, "pending");
    assert_eq!(detail.validation_items[0].status, "passed");
    assert_eq!(detail.validation_items[1].status, "pending");
    assert_eq!(detail.task.validated_base_commit, None);
}

#[tokio::test]
async fn refine_backlog_task_rejects_non_backlog_and_invalid_requirement_edits() {
    let (_temp, pool) = migrated_pool().await;
    let actor = Actor::operator("tester");
    create_task_queue(&pool, &sample_queue("TASK", "Spool"), &actor)
        .await
        .expect("create queue");
    create_task(&pool, &sample_task("TASK", "Ready"), &actor)
        .await
        .expect("create ready task");

    let input = RefineBacklogTask {
        title: None,
        brief: Some("Refined".to_string()),
        priority: None,
        target_state: None,
        review_required: None,
        acceptance_criteria: vec![],
        validation_items: vec![],
        tags: None,
        conflict_hints: None,
        blocking_task_identifiers: None,
    };
    let error = refine_backlog_task(&pool, "TASK-1", &input, &delegating_actor())
        .await
        .expect_err("non-backlog refinement rejected");
    assert!(error.to_string().contains("only supports Backlog Tasks"));

    let mut backlog = sample_task("TASK", "Backlog");
    backlog.state = "backlog".to_string();
    backlog.acceptance_criteria = vec!["First".to_string(), "Second".to_string()];
    create_task(&pool, &backlog, &actor)
        .await
        .expect("create backlog task");
    let error = refine_backlog_task(
        &pool,
        "TASK-2",
        &RefineBacklogTask {
            acceptance_criteria: vec!["Only one".to_string()],
            ..input
        },
        &delegating_actor(),
    )
    .await
    .expect_err("requirement removal rejected");
    assert!(error.to_string().contains("cannot remove"));
}

#[tokio::test]
async fn mutations_require_attributed_actor() {
    let (_temp, pool) = migrated_pool().await;
    let error = create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor {
            kind: "operator".to_string(),
            id: "".to_string(),
            display_name: "tester".to_string(),
        },
    )
    .await
    .expect_err("blank actor id fails");

    assert!(error.to_string().contains("Actor id must not be blank"));
}

#[tokio::test]
async fn task_creation_appends_audit_event() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");

    let task = create_task(
        &pool,
        &sample_task("TASK", "Audited"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");

    let events = list_audit_events(&pool).await.expect("audit events");
    assert_eq!(events.len(), 2);
    let event = events
        .iter()
        .find(|event| event.event_type == "task.created")
        .expect("task.created event");
    assert_eq!(event.subject_id, task.task.id);
    assert!(event.payload_json.contains("TASK-1"));
}

#[tokio::test]
async fn task_conflict_hints_are_stored_and_render_ready_in_progress_overlaps() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");

    let mut first = sample_task("TASK", "First");
    first.conflict_hints = vec!["AGENTS.md".to_string(), "CONTEXT.md".to_string()];
    create_task(&pool, &first, &Actor::operator("tester"))
        .await
        .expect("create first");
    let mut second = sample_task("TASK", "Second");
    second.conflict_hints = vec!["AGENTS.md".to_string()];
    create_task(&pool, &second, &Actor::operator("tester"))
        .await
        .expect("create second");
    let mut backlog = sample_task("TASK", "Backlog");
    backlog.state = "backlog".to_string();
    backlog.conflict_hints = vec!["CONTEXT.md".to_string()];
    create_task(&pool, &backlog, &Actor::operator("tester"))
        .await
        .expect("create backlog");

    let detail = get_task_detail(&pool, "TASK-1")
        .await
        .expect("load task")
        .expect("task exists");
    assert_eq!(
        detail
            .conflict_hints
            .iter()
            .map(|hint| hint.target.as_str())
            .collect::<Vec<_>>(),
        vec!["AGENTS.md", "CONTEXT.md"]
    );
    assert_eq!(detail.conflict_overlaps.len(), 1);
    assert_eq!(detail.conflict_overlaps[0].target, "AGENTS.md");
    assert_eq!(detail.conflict_overlaps[0].task_identifier, "TASK-2");

    let groups = task_conflict_groups_for_status(&pool)
        .await
        .expect("conflict groups");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].target, "AGENTS.md");
    assert_eq!(groups[0].task_count, 2);
    assert!(groups[0].tasks.contains("TASK-1 (ready)"));
    assert!(groups[0].tasks.contains("TASK-2 (ready)"));

    let claimed = claim_next(
        &pool,
        &ClaimNextInput {
            queue_key: "TASK".to_string(),
            worker_id: "worker".to_string(),
            launcher_kind: "fake".to_string(),
            lease_seconds: 60,
        },
        &Actor {
            kind: "worker_agent".to_string(),
            id: "worker".to_string(),
            display_name: "Worker".to_string(),
        },
    )
    .await
    .expect("claim despite advisory Task Conflict Hints");
    assert!(
        claimed.is_some(),
        "Task Conflict Hints must not block claims"
    );
}

#[tokio::test]
async fn creates_child_tasks_in_parent_queue_and_records_relationships() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Parent"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create parent");

    let child = create_child_task(
        &pool,
        "TASK-1",
        &CreateChildTask {
            title: "Child".to_string(),
            brief: "Child work".to_string(),
            priority: "normal".to_string(),
            state: "ready".to_string(),
            review_required: false,
            acceptance_criteria: vec!["Child works".to_string()],
            validation_items: vec!["Tests pass".to_string()],
            tags: vec!["child".to_string()],
            conflict_hints: vec![],
            blocks_parent: true,
        },
        &worker_actor(),
    )
    .await
    .expect("create child");

    assert_eq!(child.task.identifier, "TASK-2");
    assert_eq!(child.task.task_queue_key, "TASK");
    let relationships: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_relationships")
        .fetch_one(&pool)
        .await
        .expect("relationship count");
    assert_eq!(relationships, 2);
    assert!(list_task_audit_events(&pool, "TASK-1")
        .await
        .unwrap()
        .iter()
        .any(|event| event.event_type == "task.child_created"));
}

#[tokio::test]
async fn child_task_creation_rejects_bad_actor_and_invalid_ready_requirements() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Parent"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create parent");
    let mut child = CreateChildTask {
        title: "Child".to_string(),
        brief: "Child work".to_string(),
        priority: "normal".to_string(),
        state: "ready".to_string(),
        review_required: false,
        acceptance_criteria: vec![],
        validation_items: vec![],
        tags: vec![],
        conflict_hints: vec![],
        blocks_parent: false,
    };

    let error = create_child_task(&pool, "TASK-1", &child, &worker_actor())
        .await
        .expect_err("ready child without requirements fails");
    assert!(error.to_string().contains("Ready Tasks require"));

    child.state = "backlog".to_string();
    let error = create_child_task(
        &pool,
        "TASK-1",
        &child,
        &Actor {
            kind: "review_agent".to_string(),
            id: "reviewer".to_string(),
            display_name: "reviewer".to_string(),
        },
    )
    .await
    .expect_err("bad actor fails");
    assert!(error.to_string().contains("Child Task creation requires"));
}

#[tokio::test]
async fn task_links_are_upserted_with_one_primary_handoff_link() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Links"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");

    let detail = upsert_task_link(
        &pool,
        "TASK-1",
        &UpsertTaskLink {
            kind: "local_worktree".to_string(),
            target: "/tmp/worktrees/TASK-1".to_string(),
            label: Some("Local Worktree".to_string()),
            is_primary: true,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("upsert worktree link");
    assert_eq!(detail.task_links.len(), 1);
    assert!(detail.task_links[0].is_primary);

    let detail = upsert_task_link(
        &pool,
        "TASK-1",
        &UpsertTaskLink {
            kind: "task_branch".to_string(),
            target: "spool/TASK-1".to_string(),
            label: Some("Task Branch".to_string()),
            is_primary: true,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("upsert branch link");

    assert_eq!(detail.task_links.len(), 2);
    assert_eq!(
        detail
            .task_links
            .iter()
            .filter(|link| link.is_primary)
            .count(),
        1
    );
    assert_eq!(
        detail
            .task_links
            .iter()
            .find(|link| link.is_primary)
            .unwrap()
            .kind,
        "task_branch"
    );
    assert!(list_task_audit_events(&pool, "TASK-1")
        .await
        .unwrap()
        .iter()
        .any(|event| event.event_type == "task_link.upserted"));
}

#[tokio::test]
async fn workpad_note_updates_create_revisions_and_audit_events() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Workpad"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");

    let first = update_workpad_note(&pool, "TASK-1", "first note", &Actor::operator("tester"))
        .await
        .expect("first update");
    let second = update_workpad_note(&pool, "TASK-1", "second note", &Actor::operator("tester"))
        .await
        .expect("second update");

    assert_eq!(first.workpad_note.unwrap().body, "first note");
    assert_eq!(second.workpad_note.unwrap().body, "second note");
    assert_eq!(count_workpad_revisions(&pool, "TASK-1").await.unwrap(), 1);
    let events = list_audit_events(&pool).await.expect("audit events");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "workpad_note.updated")
            .count(),
        2
    );
}

#[tokio::test]
async fn transition_task_state_enforces_gates_and_audit_events() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Transition"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed");

    let error = transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "integrating".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &worker_actor(),
    )
    .await
    .expect_err("gates fail");
    assert!(error.to_string().contains("pass gates"));

    update_acceptance_criterion_status(
        &pool,
        "TASK-1",
        1,
        &UpdateRequirementStatus {
            status: "satisfied".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &worker_actor(),
    )
    .await
    .expect("criterion");
    update_validation_item_status(
        &pool,
        "TASK-1",
        1,
        &UpdateRequirementStatus {
            status: "passed".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &worker_actor(),
    )
    .await
    .expect("validation");

    let detail = transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "integrating".to_string(),
            agent_run_id: Some(claimed.run.id.clone()),
            repair_override: false,
        },
        &worker_actor(),
    )
    .await
    .expect("transition");
    assert_eq!(detail.task.state, "integrating");
    assert!(list_task_audit_events(&pool, "TASK-1")
        .await
        .unwrap()
        .iter()
        .any(|event| event.event_type == "task.state_transitioned"));
}

#[tokio::test]
async fn transition_task_state_requires_requirements_before_ready() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    let mut task = sample_task("TASK", "Sparse backlog");
    task.state = "backlog".to_string();
    task.acceptance_criteria.clear();
    task.validation_items.clear();
    create_task(&pool, &task, &Actor::operator("tester"))
        .await
        .expect("create backlog task");

    let error = transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "ready".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect_err("ready without requirements fails");

    assert!(error.to_string().contains("Ready Tasks require"));
}

#[tokio::test]
async fn transition_task_state_rejects_noop_transition_without_clearing_retry_hold() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Held"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed");
    finish_run(
        &pool,
        &claimed.run.id,
        &FinishRunInput {
            outcome: "failed".to_string(),
            failure_reason: Some("failed".to_string()),
            failure_reason_code: None,
            retry_hold_seconds: Some(60),
        },
        &worker_actor(),
    )
    .await
    .expect("failed run");

    let error = transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "in_progress".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect_err("noop fails");
    assert!(error.to_string().contains("already in requested"));
    assert!(claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .is_none());
}

#[tokio::test]
async fn cancel_transition_cancels_active_agent_runs() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Cancel"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed");

    transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "canceled".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("cancel");

    let run = get_agent_run(&pool, &claimed.run.id)
        .await
        .expect("load run")
        .expect("run exists");
    assert_eq!(run.outcome.as_deref(), Some("canceled"));
}

#[tokio::test]
async fn worker_agent_transitions_require_active_claim_lease() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "No lease"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "in_progress".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("start task");

    let error = transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "canceled".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &worker_actor(),
    )
    .await
    .expect_err("worker without lease fails");
    assert!(error.to_string().contains("active Agent Run ID"));
}

#[tokio::test]
async fn in_progress_to_done_is_allowed_when_gates_pass() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Done"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "in_progress".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("start");
    update_acceptance_criterion_status(
        &pool,
        "TASK-1",
        1,
        &UpdateRequirementStatus {
            status: "satisfied".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("criterion");
    update_validation_item_status(
        &pool,
        "TASK-1",
        1,
        &UpdateRequirementStatus {
            status: "passed".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("validation");

    let detail = transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "done".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("done");
    assert_eq!(detail.task.state, "done");
}

#[tokio::test]
async fn transition_task_state_rejects_invalid_edges_and_worker_review_required_integrating() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    let mut task = sample_task("TASK", "Review required");
    task.review_required = true;
    create_task(&pool, &task, &Actor::operator("tester"))
        .await
        .expect("create task");

    let invalid = transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "done".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect_err("ready to done invalid");
    assert!(invalid.to_string().contains("not allowed"));

    claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed");
    update_acceptance_criterion_status(
        &pool,
        "TASK-1",
        1,
        &UpdateRequirementStatus {
            status: "satisfied".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &worker_actor(),
    )
    .await
    .expect("criterion");
    update_validation_item_status(
        &pool,
        "TASK-1",
        1,
        &UpdateRequirementStatus {
            status: "passed".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &worker_actor(),
    )
    .await
    .expect("validation");
    let forbidden = transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "integrating".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &worker_actor(),
    )
    .await
    .expect_err("worker cannot integrate review required");
    assert!(forbidden.to_string().contains("Review-required"));
}

#[tokio::test]
async fn review_decision_approve_moves_human_review_task_to_integrating_with_audit() {
    let (_temp, pool) = migrated_pool().await;
    create_human_review_task(&pool, true).await;
    let actor = Actor {
        kind: "review_agent".to_string(),
        id: "reviewer".to_string(),
        display_name: "reviewer".to_string(),
    };

    let detail = record_review_decision(
        &pool,
        "TASK-1",
        &RecordReviewDecision {
            decision: "approve".to_string(),
            feedback: None,
        },
        &actor,
    )
    .await
    .expect("approve Review Decision");

    assert_eq!(detail.task.state, "integrating");
    let events = list_task_audit_events(&pool, "TASK-1")
        .await
        .expect("audit events");
    let review_event = events
        .iter()
        .find(|event| event.event_type == "task.review_decision_recorded")
        .expect("review decision audit event");
    assert_eq!(review_event.actor_kind, "review_agent");
    assert!(review_event
        .payload_json
        .contains("\"decision\":\"approve\""));
    assert!(events.iter().any(|event| {
        event.event_type == "task.state_transitioned"
            && event.payload_json.contains("\"to\":\"integrating\"")
    }));
}

#[tokio::test]
async fn review_decision_rework_moves_to_rework_and_captures_feedback() {
    let (_temp, pool) = migrated_pool().await;
    create_human_review_task(&pool, true).await;

    let detail = record_review_decision(
        &pool,
        "TASK-1",
        &RecordReviewDecision {
            decision: "rework".to_string(),
            feedback: Some("Please tighten the deterministic tests.".to_string()),
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("rework Review Decision");

    assert_eq!(detail.task.state, "rework");
    let workpad = detail
        .workpad_note
        .expect("feedback captured in Workpad Note");
    assert!(workpad.body.contains("Review Decision: Rework"));
    assert!(workpad.body.contains("Please tighten"));
    let events = list_task_audit_events(&pool, "TASK-1")
        .await
        .expect("audit events");
    assert!(events
        .iter()
        .any(|event| event.event_type == "workpad_note.updated"));
    assert!(events.iter().any(|event| {
        event.event_type == "task.review_decision_recorded"
            && event.actor_kind == "operator"
            && event.payload_json.contains("\"decision\":\"rework\"")
    }));
}

#[tokio::test]
async fn review_decision_rejects_invalid_state_gate_failure_and_actor() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Ready"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let invalid_state = record_review_decision(
        &pool,
        "TASK-1",
        &RecordReviewDecision {
            decision: "approve".to_string(),
            feedback: None,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect_err("not in Human Review");
    assert!(invalid_state.to_string().contains("Human Review"));

    transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "in_progress".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("start");
    transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "human_review".to_string(),
            agent_run_id: None,
            repair_override: true,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("operator repair to Human Review for gate failure test");

    let gate_failure = record_review_decision(
        &pool,
        "TASK-1",
        &RecordReviewDecision {
            decision: "approve".to_string(),
            feedback: None,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect_err("approve gates fail");
    assert!(gate_failure.to_string().contains("pass gates"));

    let worker = record_review_decision(
        &pool,
        "TASK-1",
        &RecordReviewDecision {
            decision: "rework".to_string(),
            feedback: Some("feedback".to_string()),
        },
        &worker_actor(),
    )
    .await
    .expect_err("worker cannot record Review Decision");
    assert!(worker.to_string().contains("Review Decisions require"));
}

#[tokio::test]
async fn updates_requirement_statuses_and_audit_events() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Requirements"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");

    let detail = update_acceptance_criterion_status(
        &pool,
        "TASK-1",
        1,
        &UpdateRequirementStatus {
            status: "satisfied".to_string(),
            waiver_reason: Some("ignored".to_string()),
            validated_base_commit: None,
        },
        &Actor {
            kind: "worker_agent".to_string(),
            id: "worker".to_string(),
            display_name: "worker".to_string(),
        },
    )
    .await
    .expect("update criterion");
    assert_eq!(detail.acceptance_criteria[0].status, "satisfied");
    assert_eq!(detail.acceptance_criteria[0].waiver_reason, None);

    let detail = update_validation_item_status(
        &pool,
        "TASK-1",
        1,
        &UpdateRequirementStatus {
            status: "passed".to_string(),
            waiver_reason: None,
            validated_base_commit: Some("abc123".to_string()),
        },
        &Actor {
            kind: "worker_agent".to_string(),
            id: "worker".to_string(),
            display_name: "worker".to_string(),
        },
    )
    .await
    .expect("update validation");
    assert_eq!(detail.validation_items[0].status, "passed");
    assert_eq!(detail.task.validated_base_commit.as_deref(), Some("abc123"));

    let events = list_task_audit_events(&pool, "TASK-1")
        .await
        .expect("task audit events");
    assert!(events
        .iter()
        .any(|event| event.event_type == "acceptance_criterion.status_updated"));
    assert!(events
        .iter()
        .any(|event| event.event_type == "validation_item.status_updated"));
}

#[tokio::test]
async fn records_validated_base_commit_on_validation_status_update() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Requirements"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");

    let detail = update_validation_item_status(
        &pool,
        "TASK-1",
        1,
        &UpdateRequirementStatus {
            status: "passed".to_string(),
            waiver_reason: None,
            validated_base_commit: Some("abc123".to_string()),
        },
        &worker_actor(),
    )
    .await
    .expect("update validation");

    assert_eq!(detail.validation_items[0].status, "passed");
    assert_eq!(detail.task.validated_base_commit.as_deref(), Some("abc123"));
}

#[tokio::test]
async fn waivers_require_allowed_actor_and_reason() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Waivers"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let worker = Actor {
        kind: "worker_agent".to_string(),
        id: "worker".to_string(),
        display_name: "worker".to_string(),
    };
    let waiver = UpdateRequirementStatus {
        status: "waived".to_string(),
        waiver_reason: Some("not needed".to_string()),
        validated_base_commit: None,
    };

    let error = update_acceptance_criterion_status(&pool, "TASK-1", 1, &waiver, &worker)
        .await
        .expect_err("worker waiver fails");
    assert!(error
        .to_string()
        .contains("Worker Agents cannot create Waivers"));

    let missing_reason = UpdateRequirementStatus {
        status: "waived".to_string(),
        waiver_reason: Some(" ".to_string()),
        validated_base_commit: None,
    };
    let error = update_acceptance_criterion_status(
        &pool,
        "TASK-1",
        1,
        &missing_reason,
        &Actor::operator("tester"),
    )
    .await
    .expect_err("missing reason fails");
    assert!(error.to_string().contains("explicit reason"));

    let detail =
        update_acceptance_criterion_status(&pool, "TASK-1", 1, &waiver, &Actor::operator("tester"))
            .await
            .expect("operator waiver succeeds");
    assert_eq!(detail.acceptance_criteria[0].status, "waived");
    assert_eq!(
        detail.acceptance_criteria[0].waiver_reason.as_deref(),
        Some("not needed")
    );
}

#[tokio::test]
async fn blocking_tasks_are_recorded_and_reject_cross_queue_and_cycles() {
    let (_temp, pool) = migrated_pool().await;
    let actor = Actor::operator("tester");
    create_task_queue(&pool, &sample_queue("TASK", "Spool"), &actor)
        .await
        .expect("create queue");
    create_task_queue(&pool, &sample_queue("OTHER", "Other"), &actor)
        .await
        .expect("create other queue");
    create_task(&pool, &sample_task("TASK", "Blocking"), &actor)
        .await
        .expect("create blocking task");
    create_task(&pool, &sample_task("OTHER", "Other blocking"), &actor)
        .await
        .expect("create other blocking task");

    let mut blocked = sample_task("TASK", "Blocked");
    blocked.blocking_task_identifiers = vec![" task-1 ".to_string(), "TASK-1".to_string()];
    let detail = create_task(&pool, &blocked, &actor)
        .await
        .expect("create blocked task");
    assert_eq!(detail.blocking_tasks.len(), 1);
    assert_eq!(detail.blocking_tasks[0].identifier, "TASK-1");
    assert!(!detail.blocking_tasks[0].resolved);

    let mut cross_queue = sample_task("TASK", "Cross queue blocked");
    cross_queue.blocking_task_identifiers = vec!["OTHER-1".to_string()];
    let error = create_task(&pool, &cross_queue, &actor)
        .await
        .expect_err("cross queue blocker rejected");
    assert!(error.to_string().contains("same Task Queue"));

    let mut tx = pool.begin().await.expect("begin tx");
    let task_1_id: String = sqlx::query_scalar("SELECT id FROM tasks WHERE identifier = 'TASK-1'")
        .fetch_one(&mut *tx)
        .await
        .expect("task 1 id");
    let task_2_id: String = sqlx::query_scalar("SELECT id FROM tasks WHERE identifier = 'TASK-2'")
        .fetch_one(&mut *tx)
        .await
        .expect("task 2 id");
    let error = crate::tasks::ensure_no_blocking_cycle(&mut tx, &task_2_id, &task_1_id)
        .await
        .expect_err("cycle rejected");
    assert!(error.to_string().contains("cycle"));
}

#[tokio::test]
async fn operator_can_add_and_remove_blocking_task_relationships_with_audit_events() {
    let (_temp, pool) = migrated_pool().await;
    let actor = Actor::operator("tester");
    create_task_queue(&pool, &sample_queue("TASK", "Spool"), &actor)
        .await
        .expect("create queue");
    create_task_queue(&pool, &sample_queue("OTHER", "Other"), &actor)
        .await
        .expect("create other queue");
    create_task(&pool, &sample_task("TASK", "Blocking"), &actor)
        .await
        .expect("create blocking task");
    create_task(&pool, &sample_task("TASK", "Blocked"), &actor)
        .await
        .expect("create blocked task");
    create_task(&pool, &sample_task("OTHER", "Other"), &actor)
        .await
        .expect("create other task");

    let detail = add_blocking_task_relationship(&pool, "task-2", " task-1 ", &actor)
        .await
        .expect("add blocker");
    assert_eq!(detail.task.identifier, "TASK-2");
    assert_eq!(detail.blocking_tasks.len(), 1);
    assert_eq!(detail.blocking_tasks[0].identifier, "TASK-1");
    assert!(!detail.blocking_tasks[0].resolved);

    let duplicate = add_blocking_task_relationship(&pool, "TASK-2", "TASK-1", &actor)
        .await
        .expect_err("duplicate blocker rejected");
    assert!(duplicate.to_string().contains("already exists"));

    let cross_queue = add_blocking_task_relationship(&pool, "TASK-2", "OTHER-1", &actor)
        .await
        .expect_err("cross queue blocker rejected");
    assert!(cross_queue.to_string().contains("same Task Queue"));

    let cycle = add_blocking_task_relationship(&pool, "TASK-1", "TASK-2", &actor)
        .await
        .expect_err("cycle rejected");
    assert!(cycle.to_string().contains("cycle"));

    let removed = remove_blocking_task_relationship(&pool, "TASK-2", "TASK-1", &actor)
        .await
        .expect("remove blocker");
    assert!(removed.blocking_tasks.is_empty());

    let missing = remove_blocking_task_relationship(&pool, "TASK-2", "TASK-1", &actor)
        .await
        .expect_err("missing relationship rejected");
    assert!(missing.to_string().contains("not found"));

    let events = list_task_audit_events(&pool, "TASK-2")
        .await
        .expect("audit events");
    assert!(events
        .iter()
        .any(|event| event.event_type == "task.blocking_task.added"
            && event.actor_display_name == "tester"
            && event.payload_json.contains("TASK-1")));
    assert!(events
        .iter()
        .any(|event| event.event_type == "task.blocking_task.removed"
            && event.actor_kind == "operator"));
}

#[tokio::test]
async fn claim_next_skips_blocked_tasks_until_blocking_tasks_are_done() {
    let (_temp, pool) = migrated_pool().await;
    let actor = Actor::operator("tester");
    create_task_queue(&pool, &sample_queue("TASK", "Spool"), &actor)
        .await
        .expect("create queue");
    create_task(&pool, &sample_task("TASK", "Blocking"), &actor)
        .await
        .expect("create blocking task");
    let mut blocked = sample_task("TASK", "Blocked");
    blocked.blocking_task_identifiers = vec!["TASK-1".to_string()];
    create_task(&pool, &blocked, &actor)
        .await
        .expect("create blocked task");

    let first = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("blocking task claimed");
    assert_eq!(first.task.task.identifier, "TASK-1");
    assert!(claim_next(
        &pool,
        &ClaimNextInput {
            worker_id: "worker-2".to_string(),
            ..sample_claim("TASK")
        },
        &Actor {
            kind: "worker_agent".to_string(),
            id: "worker-2".to_string(),
            display_name: "worker 2".to_string(),
        },
    )
    .await
    .expect("claim blocked")
    .is_none());

    update_acceptance_criterion_status(
        &pool,
        "TASK-1",
        1,
        &UpdateRequirementStatus {
            status: "satisfied".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &worker_actor(),
    )
    .await
    .expect("criterion");
    update_validation_item_status(
        &pool,
        "TASK-1",
        1,
        &UpdateRequirementStatus {
            status: "passed".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &worker_actor(),
    )
    .await
    .expect("validation");
    transition_task_state(
        &pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "done".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &actor,
    )
    .await
    .expect("done");
    finish_run(
        &pool,
        &first.run.id,
        &FinishRunInput {
            outcome: "completed".to_string(),
            failure_reason: None,
            failure_reason_code: None,
            retry_hold_seconds: None,
        },
        &worker_actor(),
    )
    .await
    .expect("finish run");

    let second = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("blocked task unblocked");
    assert_eq!(second.task.task.identifier, "TASK-2");
}

#[tokio::test]
async fn blocked_tasks_cannot_complete_without_done_blockers_or_repair_override() {
    let (_temp, pool) = migrated_pool().await;
    let actor = Actor::operator("tester");
    create_task_queue(&pool, &sample_queue("TASK", "Spool"), &actor)
        .await
        .expect("create queue");
    create_task(&pool, &sample_task("TASK", "Blocking"), &actor)
        .await
        .expect("create blocking task");
    let mut blocked = sample_task("TASK", "Blocked");
    blocked.blocking_task_identifiers = vec!["TASK-1".to_string()];
    create_task(&pool, &blocked, &actor)
        .await
        .expect("create blocked task");

    let detail = get_task_detail(&pool, "TASK-2")
        .await
        .expect("detail")
        .expect("task");
    assert_eq!(detail.blocking_tasks[0].identifier, "TASK-1");
    assert_eq!(detail.blocked_tasks.len(), 0);
    let blocker_detail = get_task_detail(&pool, "TASK-1")
        .await
        .expect("detail")
        .expect("task");
    assert_eq!(blocker_detail.blocked_tasks[0].identifier, "TASK-2");
    let context = get_task_context_bundle(&pool, "TASK-2")
        .await
        .expect("context")
        .expect("context task");
    assert_eq!(context.task.blocking_tasks[0].identifier, "TASK-1");
    let status_tasks = tasks_for_status_by_states(&pool, &["ready"])
        .await
        .expect("status tasks");
    let blocked_status = status_tasks
        .iter()
        .find(|task| task.identifier == "TASK-2")
        .expect("blocked task in status");
    assert_eq!(blocked_status.unresolved_blocking_task_count, 1);
    assert_eq!(
        blocked_status.blocking_task_identifiers.as_deref(),
        Some("TASK-1:ready")
    );

    transition_task_state(
        &pool,
        "TASK-2",
        &TransitionTaskState {
            to_state: "in_progress".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &actor,
    )
    .await
    .expect("start blocked manually");
    update_acceptance_criterion_status(
        &pool,
        "TASK-2",
        1,
        &UpdateRequirementStatus {
            status: "satisfied".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &actor,
    )
    .await
    .expect("criterion");
    update_validation_item_status(
        &pool,
        "TASK-2",
        1,
        &UpdateRequirementStatus {
            status: "passed".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &actor,
    )
    .await
    .expect("validation");

    for to_state in ["human_review", "integrating", "done"] {
        let error = transition_task_state(
            &pool,
            "TASK-2",
            &TransitionTaskState {
                to_state: to_state.to_string(),
                agent_run_id: None,
                repair_override: false,
            },
            &actor,
        )
        .await
        .expect_err("blocked transition fails");
        assert!(error
            .to_string()
            .contains("Blocked Tasks cannot transition"));
    }

    let worker_error = transition_task_state(
        &pool,
        "TASK-2",
        &TransitionTaskState {
            to_state: "done".to_string(),
            agent_run_id: None,
            repair_override: true,
        },
        &worker_actor(),
    )
    .await
    .expect_err("worker repair override rejected");
    assert!(worker_error.to_string().contains("Operator actor"));

    let detail = transition_task_state(
        &pool,
        "TASK-2",
        &TransitionTaskState {
            to_state: "done".to_string(),
            agent_run_id: None,
            repair_override: true,
        },
        &actor,
    )
    .await
    .expect("operator repair override");
    assert_eq!(detail.task.state, "done");
}

#[tokio::test]
async fn claim_next_claims_ready_task_and_moves_to_in_progress() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Claim me"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");

    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");

    assert_eq!(claimed.task.task.identifier, "TASK-1");
    assert_eq!(claimed.task.task.state, "in_progress");
    assert_eq!(claimed.run.outcome, None);
    assert_eq!(claimed.run.worker_actor_kind, "worker_agent");
}

#[tokio::test]
async fn claim_next_orders_by_priority_and_skips_active_runs() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    let mut normal = sample_task("TASK", "Normal");
    normal.priority = "normal".to_string();
    let mut urgent = sample_task("TASK", "Urgent");
    urgent.priority = "urgent".to_string();
    create_task(&pool, &normal, &Actor::operator("tester"))
        .await
        .expect("normal");
    create_task(&pool, &urgent, &Actor::operator("tester"))
        .await
        .expect("urgent");

    let first = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");
    let second = claim_next(
        &pool,
        &ClaimNextInput {
            worker_id: "worker-2".to_string(),
            ..sample_claim("TASK")
        },
        &Actor {
            kind: "worker_agent".to_string(),
            id: "worker-2".to_string(),
            display_name: "worker 2".to_string(),
        },
    )
    .await
    .expect("claim")
    .expect("claimed task");

    assert_eq!(first.task.task.title, "Urgent");
    assert_eq!(second.task.task.title, "Normal");
}

#[tokio::test]
async fn queue_concurrency_limit_blocks_claims_including_integrating_runs() {
    let (_temp, pool) = migrated_pool().await;
    let mut queue = sample_queue("TASK", "Spool");
    queue.queue_concurrency_limit = Some(1);
    create_task_queue(&pool, &queue, &Actor::operator("tester"))
        .await
        .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "One"),
        &Actor::operator("tester"),
    )
    .await
    .expect("one");
    create_task(
        &pool,
        &sample_task("TASK", "Two"),
        &Actor::operator("tester"),
    )
    .await
    .expect("two");

    assert!(claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .is_some());
    sqlx::query("UPDATE tasks SET state = 'integrating' WHERE identifier = 'TASK-1'")
        .execute(&pool)
        .await
        .expect("mark claimed Task Integrating");
    let active_runs = active_agent_runs_for_status(&pool)
        .await
        .expect("active runs");
    assert_eq!(active_runs[0].task_state, "integrating");
    assert!(claim_next(
        &pool,
        &ClaimNextInput {
            worker_id: "worker-2".to_string(),
            ..sample_claim("TASK")
        },
        &Actor {
            kind: "worker_agent".to_string(),
            id: "worker-2".to_string(),
            display_name: "worker 2".to_string(),
        },
    )
    .await
    .expect("claim")
    .is_none());
}

#[tokio::test]
async fn claim_next_does_not_reclaim_integrating_tasks() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Ready for merge"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    sqlx::query("UPDATE tasks SET state = 'integrating' WHERE identifier = 'TASK-1'")
        .execute(&pool)
        .await
        .expect("mark integrating");

    assert!(claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .is_none());
}

#[tokio::test]
async fn heartbeat_and_finish_run_do_not_change_task_state() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Run"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");

    let heartbeat = heartbeat_run(&pool, &claimed.run.id, 90, &worker_actor())
        .await
        .expect("heartbeat");
    assert!(heartbeat.last_heartbeat_at.is_some());
    let finished = finish_run(
        &pool,
        &claimed.run.id,
        &FinishRunInput {
            outcome: "completed".to_string(),
            failure_reason: None,
            failure_reason_code: None,
            retry_hold_seconds: None,
        },
        &worker_actor(),
    )
    .await
    .expect("finish");

    assert_eq!(finished.outcome.as_deref(), Some("completed"));
    let task = get_task_detail(&pool, "TASK-1").await.unwrap().unwrap();
    assert_eq!(task.task.state, "in_progress");
}

#[tokio::test]
async fn finish_run_rejects_expired_lease() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Run"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");
    sqlx::query(
        "UPDATE agent_runs SET lease_expires_at = datetime('now', '-1 second') WHERE id = ?",
    )
    .bind(&claimed.run.id)
    .execute(&pool)
    .await
    .expect("backdate lease");

    let error = finish_run(
        &pool,
        &claimed.run.id,
        &FinishRunInput {
            outcome: "completed".to_string(),
            failure_reason: None,
            failure_reason_code: None,
            retry_hold_seconds: None,
        },
        &worker_actor(),
    )
    .await
    .expect_err("expired lease cannot finish");
    assert!(error.to_string().contains("not found for actor"));
}

#[tokio::test]
async fn failed_run_creates_retry_hold_and_blocks_reclaim() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Run"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");
    let failed = finish_run(
        &pool,
        &claimed.run.id,
        &FinishRunInput {
            outcome: "failed".to_string(),
            failure_reason: Some("fake failure".to_string()),
            failure_reason_code: Some("launcher_exited".to_string()),
            retry_hold_seconds: Some(60),
        },
        &worker_actor(),
    )
    .await
    .expect("finish failed");
    assert_eq!(
        failed.failure_reason_code.as_deref(),
        Some("launcher_exited")
    );

    assert!(claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .is_none());
}

#[tokio::test]
async fn finish_run_rejects_invalid_failure_reason_code() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Run"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");

    let error = finish_run(
        &pool,
        &claimed.run.id,
        &FinishRunInput {
            outcome: "failed".to_string(),
            failure_reason: Some("fake failure".to_string()),
            failure_reason_code: Some("not-a-code".to_string()),
            retry_hold_seconds: Some(60),
        },
        &worker_actor(),
    )
    .await
    .expect_err("invalid code rejected");
    assert!(error
        .to_string()
        .contains("invalid Agent Run failure reason code"));
}

#[tokio::test]
async fn operator_fail_run_records_retry_hold_and_audit_events() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Run"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");

    let failed = operator_fail_run(
        &pool,
        &claimed.run.id,
        &OperatorFailRunInput {
            failure_reason: "SQLite database is locked".to_string(),
            failure_reason_code: None,
            retry_hold_seconds: Some(120),
        },
        &Actor::operator("operator"),
    )
    .await
    .expect("operator fail run");

    assert_eq!(failed.outcome.as_deref(), Some("failed"));
    assert_eq!(
        failed.failure_reason.as_deref(),
        Some("SQLite database is locked")
    );
    assert_eq!(
        failed.failure_reason_code.as_deref(),
        Some("operator_failed")
    );
    assert!(claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .is_none());
    let events = list_audit_events(&pool).await.expect("events");
    assert!(events
        .iter()
        .any(|event| event.event_type == "agent_run.operator_failed"));
    assert!(events
        .iter()
        .any(|event| event.event_type == "task.retry_hold_created"));
}

#[tokio::test]
async fn retry_task_moves_resolved_failed_task_to_ready_and_clears_hold() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Run"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");
    finish_run(
        &pool,
        &claimed.run.id,
        &FinishRunInput {
            outcome: "failed".to_string(),
            failure_reason: Some("fake failure".to_string()),
            failure_reason_code: None,
            retry_hold_seconds: Some(60),
        },
        &worker_actor(),
    )
    .await
    .expect("finish failed");

    let retried = retry_task(
        &pool,
        "TASK-1",
        &RetryTaskInput {
            reason: "operator retry after fixing local lock".to_string(),
        },
        &Actor::operator("operator"),
    )
    .await
    .expect("retry task");

    assert_eq!(retried.task.state, "ready");
    let reclaimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("reclaimed task");
    assert_eq!(reclaimed.task.task.identifier, "TASK-1");
    let events = list_audit_events(&pool).await.expect("events");
    assert!(events
        .iter()
        .any(|event| event.event_type == "task.retry_requested"));
    assert!(events
        .iter()
        .any(|event| event.event_type == "task.retry_hold_cleared"));
}

#[tokio::test]
async fn retry_task_resolves_expired_stuck_run_but_rejects_live_run() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Run"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");

    let live_error = retry_task(
        &pool,
        "TASK-1",
        &RetryTaskInput {
            reason: "too soon".to_string(),
        },
        &Actor::operator("operator"),
    )
    .await
    .expect_err("live run blocks retry");
    assert!(live_error.to_string().contains("active Agent Runs"));

    sqlx::query(
        "UPDATE agent_runs SET lease_expires_at = datetime('now', '-1 second') WHERE id = ?",
    )
    .bind(&claimed.run.id)
    .execute(&pool)
    .await
    .expect("backdate lease");

    let retried = retry_task(
        &pool,
        "TASK-1",
        &RetryTaskInput {
            reason: "lease expired".to_string(),
        },
        &Actor::operator("operator"),
    )
    .await
    .expect("retry expired task");
    assert_eq!(retried.task.state, "ready");
    let run = get_agent_run(&pool, &claimed.run.id)
        .await
        .expect("run")
        .expect("exists");
    assert_eq!(run.outcome.as_deref(), Some("expired"));
}

#[tokio::test]
async fn expired_lease_is_marked_expired_before_next_claim() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Run"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");
    sqlx::query(
        "UPDATE agent_runs SET lease_expires_at = datetime('now', '-1 second') WHERE id = ?",
    )
    .bind(&claimed.run.id)
    .execute(&pool)
    .await
    .expect("backdate lease");

    assert!(claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .is_none());
    let run = get_agent_run(&pool, &claimed.run.id)
        .await
        .expect("load run")
        .expect("run exists");
    assert_eq!(run.outcome.as_deref(), Some("expired"));
}

#[tokio::test]
async fn launcher_session_data_is_upserted_and_loaded_with_run_detail() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Run"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");

    let session = upsert_launcher_session_data(
        &pool,
        &claimed.run.id,
        &UpsertLauncherSessionData {
            launcher_kind: "pi".to_string(),
            session_id: Some("session-1".to_string()),
            model: Some("model".to_string()),
            provider: Some("provider".to_string()),
            started_at: Some(claimed.run.created_at.clone()),
            finished_at: None,
            final_status: Some("completed".to_string()),
            transcript_path: Some("/tmp/transcript.jsonl".to_string()),
            raw_json: Some("{}".to_string()),
        },
        &worker_actor(),
    )
    .await
    .expect("upsert session");
    assert_eq!(session.launcher_kind, "pi");
    let detail = get_agent_run_detail(&pool, &claimed.run.id)
        .await
        .expect("load detail")
        .expect("detail exists");
    assert_eq!(detail.task.task.identifier, "TASK-1");
    assert_eq!(
        detail
            .launcher_session_data
            .unwrap()
            .transcript_path
            .as_deref(),
        Some("/tmp/transcript.jsonl")
    );
}

#[tokio::test]
async fn persists_agent_run_metrics_for_fake_launcher_outcome() {
    let (temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Run"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");
    let transcript_path = temp.path().join("fake.jsonl");
    fs::write(
        &transcript_path,
        serde_json::json!({"launcher":"fake","agent_run_id":claimed.run.id}).to_string() + "\n",
    )
    .expect("write transcript");
    upsert_launcher_session_data(
        &pool,
        &claimed.run.id,
        &UpsertLauncherSessionData {
            launcher_kind: "fake".to_string(),
            session_id: Some(claimed.run.id.clone()),
            model: None,
            provider: None,
            started_at: Some(claimed.run.created_at.clone()),
            finished_at: None,
            final_status: Some("completed".to_string()),
            transcript_path: Some(transcript_path.display().to_string()),
            raw_json: Some(
                r#"{"usage":{"input_tokens":11,"output_tokens":7,"total_tokens":18}}"#.to_string(),
            ),
        },
        &worker_actor(),
    )
    .await
    .expect("upsert session");
    finish_run(
        &pool,
        &claimed.run.id,
        &FinishRunInput {
            outcome: "completed".to_string(),
            failure_reason: None,
            failure_reason_code: None,
            retry_hold_seconds: None,
        },
        &worker_actor(),
    )
    .await
    .expect("finish run");

    let metrics = get_agent_run_metrics(&pool, &claimed.run.id)
        .await
        .expect("metrics")
        .expect("metrics recorded");
    assert_eq!(
        metrics.derivation_version,
        CURRENT_AGENT_RUN_METRICS_DERIVATION_VERSION
    );
    assert_eq!(metrics.launcher_kind, "fake");
    assert_eq!(metrics.final_status.as_deref(), Some("completed"));
    assert_eq!(metrics.transcript_jsonl_event_count, Some(1));
    assert!(metrics.transcript_byte_size.unwrap_or_default() > 0);
    assert_eq!(metrics.input_tokens, Some(11));
    assert_eq!(metrics.output_tokens, Some(7));
    assert_eq!(metrics.total_tokens, Some(18));
}

#[tokio::test]
async fn persists_agent_run_metrics_for_pi_launcher_outcome() {
    let (temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Run"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let pi_claim = ClaimNextInput {
        launcher_kind: "pi".to_string(),
        ..sample_claim("TASK")
    };
    let claimed = claim_next(&pool, &pi_claim, &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");
    let transcript_path = temp.path().join("pi.jsonl");
    fs::write(
            &transcript_path,
            serde_json::json!({
                "launcher":"pi",
                "status":0,
                "stdout":"{\"role\":\"user\",\"content\":\"brief\"}\n{\"role\":\"assistant\",\"content\":\"plan\",\"usage\":{\"input_tokens\":30,\"output_tokens\":12,\"total_tokens\":42}}\n{\"type\":\"message_update\",\"assistantMessageEvent\":{\"partial\":{\"usage\":{\"input\":45,\"output\":15,\"totalTokens\":60,\"cacheRead\":1000,\"cacheWrite\":50}}}}\n{\"type\":\"tool_call\",\"tool_name\":\"read\",\"args\":{\"path\":\"src/lib.rs\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"read\",\"args\":{\"path\":\"src/lib.rs\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"bin/spool-local task show TASK-1\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"bin/spool-local task show TASK-1\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"spool status\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"spool status\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"edit\",\"args\":{}}\n{\"type\":\"tool_call\",\"tool_name\":\"write\",\"args\":{}}\n{\"type\":\"tool_call\",\"tool_name\":\"spool_get_task_context_bundle\",\"args\":{}}\n{\"type\":\"message_update\",\"message\":{\"content\":[{\"type\":\"toolCall\",\"id\":\"call-read-1\",\"name\":\"read\",\"partialJson\":\"\"},{\"type\":\"toolCall\",\"id\":\"call-read-1\",\"name\":\"read\",\"partialJson\":\"{\\\"path\\\":\\\"CONTEXT.md\\\"}\"},{\"type\":\"toolCall\",\"id\":\"call-read-2\",\"name\":\"read\",\"partialJson\":\"{\\\"path\\\":\\\"CONTEXT.md\\\"}\"}]}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"cargo test -p spool-db\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"git status --short\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"rg telemetry crates\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"ls crates\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"bun test\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"sqlite3 .spool/data/spool.db 'select count(*) from tasks'\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"ps aux | grep spool\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"jq '.efficiency' summary.json\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"pytest tests/test_metrics.py\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"python scripts/one_off.py\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"date\"}}\n{\"type\":\"tool_error\",\"tool_name\":\"read\",\"args\":{\"path\":\"src/lib.rs\"},\"status\":\"error\"}\n{\"type\":\"tool_error\",\"tool_name\":\"read\",\"args\":{\"path\":\"src/lib.rs\"},\"status\":\"error\"}\n{\"type\":\"agent_end\"}\n",
                "stderr":"",
                "unattended_question_detected":false,
                "timed_out":false
            })
            .to_string()
                + "\n",
        )
        .expect("write transcript");
    upsert_launcher_session_data(
        &pool,
        &claimed.run.id,
        &UpsertLauncherSessionData {
            launcher_kind: "pi".to_string(),
            session_id: Some(claimed.run.id.clone()),
            model: None,
            provider: None,
            started_at: Some(claimed.run.created_at.clone()),
            finished_at: None,
            final_status: Some("completed".to_string()),
            transcript_path: Some(transcript_path.display().to_string()),
            raw_json: Some(
                r#"{"exit_code":0,"timed_out":false,"unattended_question_detected":false}"#
                    .to_string(),
            ),
        },
        &worker_actor(),
    )
    .await
    .expect("upsert session");
    finish_run(
        &pool,
        &claimed.run.id,
        &FinishRunInput {
            outcome: "completed".to_string(),
            failure_reason: None,
            failure_reason_code: None,
            retry_hold_seconds: None,
        },
        &worker_actor(),
    )
    .await
    .expect("finish run");

    let metrics = get_agent_run_metrics(&pool, &claimed.run.id)
        .await
        .expect("metrics")
        .expect("metrics recorded");
    assert_eq!(
        metrics.derivation_version,
        CURRENT_AGENT_RUN_METRICS_DERIVATION_VERSION
    );
    assert_eq!(metrics.launcher_kind, "pi");
    assert_eq!(metrics.exit_code, Some(0));
    assert_eq!(metrics.timed_out, Some(0));
    assert_eq!(metrics.unattended_question_detected, Some(0));
    assert_eq!(metrics.transcript_jsonl_event_count, Some(1));
    assert_eq!(metrics.tool_call_count, Some(22));
    assert_eq!(metrics.tool_error_count, Some(2));
    assert_eq!(metrics.repeated_failed_tool_attempt_count, Some(1));
    assert_eq!(metrics.repeated_read_count, Some(2));
    assert_eq!(metrics.repeated_spool_context_fetch_count, Some(2));
    let tool_counts: std::collections::BTreeMap<String, i64> =
        serde_json::from_str(&metrics.tool_call_counts_json).expect("tool counts");
    assert_eq!(tool_counts.get("read"), Some(&4));
    assert_eq!(tool_counts.get("bash"), Some(&15));
    assert_eq!(tool_counts.get("edit"), Some(&1));
    assert_eq!(tool_counts.get("write"), Some(&1));
    assert_eq!(tool_counts.get("spool_get_task_context_bundle"), Some(&1));
    let shell_counts: std::collections::BTreeMap<String, i64> =
        serde_json::from_str(&metrics.shell_command_counts_json).expect("shell counts");
    assert_eq!(shell_counts.get("spool_cli"), Some(&4));
    assert_eq!(shell_counts.get("cargo_build_test"), Some(&1));
    assert_eq!(shell_counts.get("git"), Some(&1));
    assert_eq!(shell_counts.get("search"), Some(&1));
    assert_eq!(shell_counts.get("file_inspection"), Some(&1));
    assert_eq!(shell_counts.get("package_manager"), Some(&1));
    assert_eq!(shell_counts.get("database"), Some(&1));
    assert_eq!(shell_counts.get("process_supervisor"), Some(&1));
    assert_eq!(shell_counts.get("text_processing"), Some(&1));
    assert_eq!(shell_counts.get("test_runner"), Some(&1));
    assert_eq!(shell_counts.get("scripting"), Some(&1));
    assert_eq!(shell_counts.get("miscellaneous"), Some(&1));
    assert_eq!(shell_counts.get("other"), None);
    assert_eq!(metrics.assistant_turn_count, Some(1));
    assert_eq!(metrics.user_turn_count, Some(1));
    assert_eq!(metrics.input_tokens, Some(45));
    assert_eq!(metrics.output_tokens, Some(15));
    assert_eq!(metrics.total_tokens, Some(60));
    assert_eq!(metrics.cache_read_tokens, Some(1000));
    assert_eq!(metrics.cache_write_tokens, Some(50));
    assert_eq!(metrics.max_context_tokens, Some(60));
    let hints: Vec<String> = serde_json::from_str(&metrics.efficiency_hints_json).expect("hints");
    assert!(hints
        .iter()
        .any(|hint| hint == "repeated failed tool attempts"));
    assert!(hints.iter().any(|hint| hint == "repeated file reads"));
    assert!(hints
        .iter()
        .any(|hint| hint == "repeated Spool context fetches"));
}

#[tokio::test]
async fn agent_run_metrics_separate_blocking_ui_and_unattended_questions_from_hints() {
    let (temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");

    async fn record_run(
        pool: &SqlitePool,
        temp_path: &std::path::Path,
        title: &str,
        stdout: &str,
        final_status: &str,
        raw_json: &str,
    ) -> AgentRunMetrics {
        create_task(
            pool,
            &sample_task("TASK", title),
            &Actor::operator("tester"),
        )
        .await
        .expect("create task");
        let pi_claim = ClaimNextInput {
            launcher_kind: "pi".to_string(),
            ..sample_claim("TASK")
        };
        let claimed = claim_next(pool, &pi_claim, &worker_actor())
            .await
            .expect("claim")
            .expect("claimed task");
        let transcript_path = temp_path.join(format!("{}.jsonl", claimed.run.id));
        fs::write(
            &transcript_path,
            serde_json::json!({
                "launcher": "pi",
                "status": if final_status == "completed" { 0 } else { 1 },
                "stdout": stdout,
                "stderr": "",
                "timed_out": false
            })
            .to_string()
                + "\n",
        )
        .expect("write transcript");
        upsert_launcher_session_data(
            pool,
            &claimed.run.id,
            &UpsertLauncherSessionData {
                launcher_kind: "pi".to_string(),
                session_id: Some(claimed.run.id.clone()),
                model: None,
                provider: None,
                started_at: Some(claimed.run.created_at.clone()),
                finished_at: None,
                final_status: Some(final_status.to_string()),
                transcript_path: Some(transcript_path.display().to_string()),
                raw_json: Some(raw_json.to_string()),
            },
            &worker_actor(),
        )
        .await
        .expect("upsert session");
        finish_run(
            pool,
            &claimed.run.id,
            &FinishRunInput {
                outcome: final_status.to_string(),
                failure_reason: None,
                failure_reason_code: None,
                retry_hold_seconds: None,
            },
            &worker_actor(),
        )
        .await
        .expect("finish run");
        get_agent_run_metrics(pool, &claimed.run.id)
            .await
            .expect("metrics")
            .expect("metrics recorded")
    }

    let benign = record_run(
        &pool,
        temp.path(),
        "Benign UI",
        "{\"type\":\"extension_ui_request\",\"method\":\"notify\"}\n{\"event\":\"question\"}\n{\"type\":\"agent_end\"}\n",
        "completed",
        r#"{"exit_code":0,"timed_out":false,"unattended_question_detected":false}"#,
    )
    .await;
    assert_eq!(benign.unattended_question_detected, Some(0));
    assert_eq!(benign.blocking_ui_detected, None);
    let benign_hints: Vec<String> =
        serde_json::from_str(&benign.efficiency_hints_json).expect("benign hints");
    assert!(!benign_hints
        .iter()
        .any(|hint| hint.contains("UI") || hint.contains("question")));

    let question_failure = record_run(
        &pool,
        temp.path(),
        "Question failure",
        "{\"event\":\"question\"}\n",
        "failed",
        r#"{"exit_code":1,"timed_out":false,"unattended_question_detected":false}"#,
    )
    .await;
    assert_eq!(question_failure.unattended_question_detected, Some(1));
    assert_eq!(question_failure.blocking_ui_detected, None);

    let blocking_ui = record_run(
        &pool,
        temp.path(),
        "Blocking UI",
        "{\"type\":\"extension_ui_request\",\"method\":\"confirm\"}\n",
        "failed",
        r#"{"exit_code":1,"timed_out":false,"unattended_question_detected":false}"#,
    )
    .await;
    assert_eq!(blocking_ui.unattended_question_detected, Some(0));
    assert_eq!(blocking_ui.blocking_ui_detected, Some(1));
}

#[tokio::test]
async fn agent_run_metrics_warn_for_missing_and_malformed_transcripts() {
    let (temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Run"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim")
        .expect("claimed task");
    let missing_path = temp.path().join("missing.jsonl");
    upsert_launcher_session_data(
        &pool,
        &claimed.run.id,
        &UpsertLauncherSessionData {
            launcher_kind: "fake".to_string(),
            session_id: Some(claimed.run.id.clone()),
            model: None,
            provider: None,
            started_at: Some(claimed.run.created_at.clone()),
            finished_at: None,
            final_status: Some("failed".to_string()),
            transcript_path: Some(missing_path.display().to_string()),
            raw_json: Some("not-json".to_string()),
        },
        &worker_actor(),
    )
    .await
    .expect("upsert session");
    finish_run(
        &pool,
        &claimed.run.id,
        &FinishRunInput {
            outcome: "failed".to_string(),
            failure_reason: Some("failed".to_string()),
            failure_reason_code: None,
            retry_hold_seconds: Some(60),
        },
        &worker_actor(),
    )
    .await
    .expect("finish run");
    let missing_metrics = get_agent_run_metrics(&pool, &claimed.run.id)
        .await
        .expect("metrics")
        .expect("metrics recorded");
    assert!(missing_metrics
        .warnings_json
        .contains("could not read Run Transcript"));
    assert!(missing_metrics
        .warnings_json
        .contains("ignored malformed Launcher Session Data raw JSON"));

    create_task(
        &pool,
        &sample_task("TASK", "Run 2"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task 2");
    let claimed = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim 2")
        .expect("claimed task 2");
    let malformed_path = temp.path().join("malformed.jsonl");
    fs::write(&malformed_path, "not json\n{\"launcher\":\"fake\"}\n")
        .expect("write malformed transcript");
    upsert_launcher_session_data(
        &pool,
        &claimed.run.id,
        &UpsertLauncherSessionData {
            launcher_kind: "fake".to_string(),
            session_id: Some(claimed.run.id.clone()),
            model: None,
            provider: None,
            started_at: Some(claimed.run.created_at.clone()),
            finished_at: None,
            final_status: Some("completed".to_string()),
            transcript_path: Some(malformed_path.display().to_string()),
            raw_json: Some("{}".to_string()),
        },
        &worker_actor(),
    )
    .await
    .expect("upsert session 2");
    finish_run(
        &pool,
        &claimed.run.id,
        &FinishRunInput {
            outcome: "completed".to_string(),
            failure_reason: None,
            failure_reason_code: None,
            retry_hold_seconds: None,
        },
        &worker_actor(),
    )
    .await
    .expect("finish run 2");
    let malformed_metrics = get_agent_run_metrics(&pool, &claimed.run.id)
        .await
        .expect("metrics 2")
        .expect("metrics recorded 2");
    assert_eq!(malformed_metrics.transcript_jsonl_event_count, Some(1));
    assert!(malformed_metrics
        .warnings_json
        .contains("ignored malformed Run Transcript line 1"));
}

#[tokio::test]
async fn status_counts_tasks_by_queue_and_state() {
    let (_temp, pool) = migrated_pool().await;
    create_task_queue(
        &pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "Ready"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create ready");
    let mut backlog = sample_task("TASK", "Backlog");
    backlog.state = "backlog".to_string();
    backlog.acceptance_criteria.clear();
    backlog.validation_items.clear();
    create_task(&pool, &backlog, &Actor::operator("tester"))
        .await
        .expect("create backlog");

    let status = status_by_queue_and_state(&pool).await.expect("status");
    assert!(status
        .iter()
        .all(|row| row.queue_concurrency_limit.is_none()));
    assert!(status
        .iter()
        .any(|row| row.queue_key == "TASK" && row.state == "ready" && row.task_count == 1));
    assert!(status
        .iter()
        .any(|row| row.queue_key == "TASK" && row.state == "backlog" && row.task_count == 1));
}

#[tokio::test]
async fn status_lists_active_runs_and_retry_holds() {
    let (_temp, pool) = migrated_pool().await;
    let mut queue = sample_queue("TASK", "Spool");
    queue.queue_concurrency_limit = Some(1);
    create_task_queue(&pool, &queue, &Actor::operator("tester"))
        .await
        .expect("create queue");
    create_task(
        &pool,
        &sample_task("TASK", "First"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create first");
    create_task(
        &pool,
        &sample_task("TASK", "Second"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create second");

    let failed_run = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim first")
        .expect("first claimed");
    finish_run(
        &pool,
        &failed_run.run.id,
        &FinishRunInput {
            outcome: "failed".to_string(),
            failure_reason: Some("model unavailable".to_string()),
            failure_reason_code: None,
            retry_hold_seconds: Some(300),
        },
        &worker_actor(),
    )
    .await
    .expect("finish first failed");
    let active_run = claim_next(&pool, &sample_claim("TASK"), &worker_actor())
        .await
        .expect("claim second")
        .expect("second claimed");
    update_acceptance_criterion_status(
        &pool,
        &active_run.task.task.identifier,
        1,
        &UpdateRequirementStatus {
            status: "satisfied".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &worker_actor(),
    )
    .await
    .expect("satisfy criterion");
    update_validation_item_status(
        &pool,
        &active_run.task.task.identifier,
        1,
        &UpdateRequirementStatus {
            status: "passed".to_string(),
            waiver_reason: None,
            validated_base_commit: None,
        },
        &worker_actor(),
    )
    .await
    .expect("pass validation");
    transition_task_state(
        &pool,
        &active_run.task.task.identifier,
        &TransitionTaskState {
            to_state: "integrating".to_string(),
            agent_run_id: Some(active_run.run.id.clone()),
            repair_override: false,
        },
        &worker_actor(),
    )
    .await
    .expect("move to integrating");

    let status = status_by_queue_and_state(&pool).await.expect("status");
    let row = status
        .iter()
        .find(|row| row.queue_key == "TASK" && row.state == "integrating")
        .expect("integrating status row");
    assert_eq!(row.queue_concurrency_limit, Some(1));
    assert_eq!(row.ready_tasks, 0);
    assert_eq!(row.integrating_tasks, 1);
    assert_eq!(row.active_agent_runs, 1);
    assert_eq!(row.active_integrating_agent_runs, 1);

    let runs = active_agent_runs_for_status(&pool).await.expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].task_identifier, active_run.task.task.identifier);
    assert_eq!(runs[0].agent_run_id, active_run.run.id);
    assert_eq!(runs[0].launcher_kind, "fake");
    assert_eq!(runs[0].worker_id, "worker");
    assert_eq!(runs[0].lease_expires_at, active_run.run.lease_expires_at);

    let holds = active_retry_holds_for_status(&pool).await.expect("holds");
    assert_eq!(holds.len(), 1);
    assert_eq!(holds[0].task_identifier, failed_run.task.task.identifier);
    assert_eq!(holds[0].reason, "model unavailable");
    assert!(!holds[0].hold_until.is_empty());
}

#[tokio::test]
async fn delivery_records_integration_outcome_and_audit_event() {
    let (_temp, pool) = migrated_pool().await;
    let actor = Actor::operator("tester");
    create_task_queue(&pool, &sample_queue("TASK", "Spool"), &actor)
        .await
        .expect("queue");
    create_task(&pool, &sample_task("TASK", "Delivery"), &actor)
        .await
        .expect("task");

    let outcome = record_integration_outcome(
        &pool,
        &RecordIntegrationOutcomeInput {
            task_identifier: "TASK-1".to_string(),
            agent_run_id: None,
            outcome_kind: "success".to_string(),
            reason_code: "success".to_string(),
            final_commit: Some("abc123".to_string()),
            pre_merge_head: Some("def456".to_string()),
            message: None,
            retryable: false,
            retry_attempt: None,
            retry_delay_seconds: None,
        },
        &actor,
    )
    .await
    .expect("record outcome");

    assert_eq!(outcome.outcome_kind, "success");
    assert_eq!(outcome.reason_code.as_deref(), Some("success"));
    assert_eq!(outcome.final_commit.as_deref(), Some("abc123"));
    let events = list_task_audit_events(&pool, "TASK-1")
        .await
        .expect("events");
    assert!(events
        .iter()
        .any(|event| event.event_type == "integration_outcome.recorded"
            && event.payload_json.contains("\"reason_code\":\"success\"")));
}

#[tokio::test]
async fn integration_outcome_reason_code_validation_and_legacy_reads() {
    let (_temp, pool) = migrated_pool().await;
    let actor = Actor::operator("tester");
    create_task_queue(&pool, &sample_queue("TASK", "Spool"), &actor)
        .await
        .expect("queue");
    create_task(&pool, &sample_task("TASK", "Delivery"), &actor)
        .await
        .expect("task");

    let invalid = record_integration_outcome(
        &pool,
        &RecordIntegrationOutcomeInput {
            task_identifier: "TASK-1".to_string(),
            agent_run_id: None,
            outcome_kind: "operational_failure".to_string(),
            reason_code: "not_a_reason".to_string(),
            final_commit: None,
            pre_merge_head: None,
            message: Some("legacy-like failure".to_string()),
            retryable: true,
            retry_attempt: Some(1),
            retry_delay_seconds: Some(30),
        },
        &actor,
    )
    .await
    .expect_err("invalid reason code rejected");
    assert!(invalid
        .to_string()
        .contains("invalid Integration Outcome reason code"));

    sqlx::query(
            "INSERT INTO integration_outcomes (id, task_id, outcome_kind, message, retryable) SELECT 'legacy-outcome', id, 'operational_failure', 'old row', 1 FROM tasks WHERE identifier = 'TASK-1'",
        )
        .execute(&pool)
        .await
        .expect("legacy insert");
    sqlx::query("UPDATE tasks SET state = 'integrating' WHERE identifier = 'TASK-1'")
        .execute(&pool)
        .await
        .expect("integrating");

    let retries = integration_retries_for_status(&pool)
        .await
        .expect("retries");
    assert_eq!(retries[0].reason_code, "unknown_legacy");
}

#[tokio::test]
async fn check_migration_compatibility_does_not_apply_pending_migrations() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("spool.db");
    let pool = connect(&db_path).await.expect("connect");
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

    let error = check_migration_compatibility(&pool)
        .await
        .expect_err("pending migrations should be incompatible");
    assert!(error.to_string().contains("pending SQLite migrations"));

    let task_queues_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'task_queues'",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect tables");
    assert_eq!(task_queues_table, 0);
}

#[tokio::test]
async fn check_migration_compatibility_reports_applied_but_missing_migration() {
    let (_temp, pool) = migrated_pool().await;
    sqlx::query(
        r#"
            INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
            VALUES (9999, 'future_task_branch_migration', true, x'abcd', 0)
            "#,
    )
    .execute(&pool)
    .await
    .expect("insert missing migration marker");

    let error = check_migration_compatibility(&pool)
        .await
        .expect_err("missing applied migration should be incompatible");
    let message = error.to_string();
    assert!(message.contains("SQLite migration drift detected"));
    assert!(message.contains("migration 9999 was previously applied"));
    assert!(message.contains("Managed Source Repository Main Branch"));
}

async fn migrated_pool() -> (tempfile::TempDir, SqlitePool) {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("spool.db");
    let pool = connect(&db_path).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    (temp, pool)
}

fn sample_queue(key: &str, name: &str) -> CreateTaskQueue {
    CreateTaskQueue {
        key: key.to_string(),
        name: name.to_string(),
        managed_source_repository: "/repo/spool".to_string(),
        main_branch: "main".to_string(),
        worktree_root: "/worktrees".to_string(),
        branch_template: "spool/{task_identifier}".to_string(),
        done_worktree_retention: false,
        queue_concurrency_limit: None,
    }
}

fn delegating_actor() -> Actor {
    Actor {
        kind: "delegating_agent".to_string(),
        id: "delegate".to_string(),
        display_name: "Delegating Agent".to_string(),
    }
}

fn sample_delegation_draft(queue_key: &str, title: &str) -> DelegationTaskDraft {
    DelegationTaskDraft {
        queue_key: queue_key.to_string(),
        title: title.to_string(),
        brief: "A delegated Task Brief.".to_string(),
        priority: "normal".to_string(),
        initial_state: "backlog".to_string(),
        review_required: false,
        tags: Vec::new(),
        conflict_hints: Vec::new(),
        blocking_task_identifiers: Vec::new(),
        acceptance_criteria: vec!["Delegated outcome is clear".to_string()],
        validation_items: vec!["Deterministic tests pass".to_string()],
    }
}

fn worker_actor() -> Actor {
    Actor {
        kind: "worker_agent".to_string(),
        id: "worker".to_string(),
        display_name: "worker".to_string(),
    }
}

fn sample_claim(queue_key: &str) -> ClaimNextInput {
    ClaimNextInput {
        queue_key: queue_key.to_string(),
        worker_id: "worker".to_string(),
        launcher_kind: "fake".to_string(),
        lease_seconds: 90,
    }
}

async fn create_human_review_task(pool: &SqlitePool, gates_pass: bool) {
    create_task_queue(
        pool,
        &sample_queue("TASK", "Spool"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create queue");
    create_task(
        pool,
        &sample_task("TASK", "Review"),
        &Actor::operator("tester"),
    )
    .await
    .expect("create task");
    transition_task_state(
        pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "in_progress".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("start task");
    if gates_pass {
        update_acceptance_criterion_status(
            pool,
            "TASK-1",
            1,
            &UpdateRequirementStatus {
                status: "satisfied".to_string(),
                waiver_reason: None,
                validated_base_commit: None,
            },
            &Actor::operator("tester"),
        )
        .await
        .expect("criterion");
        update_validation_item_status(
            pool,
            "TASK-1",
            1,
            &UpdateRequirementStatus {
                status: "passed".to_string(),
                waiver_reason: None,
                validated_base_commit: None,
            },
            &Actor::operator("tester"),
        )
        .await
        .expect("validation");
    }
    transition_task_state(
        pool,
        "TASK-1",
        &TransitionTaskState {
            to_state: "human_review".to_string(),
            agent_run_id: None,
            repair_override: false,
        },
        &Actor::operator("tester"),
    )
    .await
    .expect("move to Human Review");
}

fn sample_task(queue_key: &str, title: &str) -> CreateTask {
    CreateTask {
        queue_key: queue_key.to_string(),
        title: title.to_string(),
        brief: "Implement the requested behavior.".to_string(),
        priority: "normal".to_string(),
        state: "ready".to_string(),
        review_required: false,
        acceptance_criteria: vec!["It works".to_string()],
        validation_items: vec!["cargo test passes".to_string()],
        tags: vec![
            "dogfood".to_string(),
            "backend".to_string(),
            "dogfood".to_string(),
        ],
        conflict_hints: vec![],
        blocking_task_identifiers: vec![],
    }
}

use std::{fs, future::Future, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    FromRow, SqlitePool,
};
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

pub const LOCAL_TOKEN_NAME: &str = "local";

const SQLITE_WRITE_RETRY_BACKOFFS: [Duration; 5] = [
    Duration::from_millis(5),
    Duration::from_millis(10),
    Duration::from_millis(20),
    Duration::from_millis(40),
    Duration::from_millis(80),
];

pub fn sqlite_url(db_path: &Path) -> String {
    format!("sqlite://{}", db_path.display())
}

pub async fn connect(db_path: &Path) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(30));

    SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .with_context(|| {
            format!(
                "failed to connect to SQLite database at {}",
                db_path.display()
            )
        })
}

pub async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    migration_source()
        .run(pool)
        .await
        .map_err(friendly_migration_error)
        .context("failed to run SQLite migrations")
}

pub async fn check_migration_compatibility(pool: &SqlitePool) -> Result<()> {
    let pending = pending_migration_versions(pool).await?;
    if !pending.is_empty() {
        anyhow::bail!(
            "Tasker database has pending SQLite migrations {:?}. Normal commands validate schema compatibility but do not apply migrations. Run `tasker db migrate` from the trusted Managed Source Repository Main Branch to upgrade the Task Backend.",
            pending
        );
    }

    Ok(())
}

pub async fn pending_migration_versions(pool: &SqlitePool) -> Result<Vec<i64>> {
    let migrations_table_exists: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM sqlite_master
        WHERE type = 'table' AND name = '_sqlx_migrations'
        "#,
    )
    .fetch_one(pool)
    .await
    .context("failed to inspect SQLite migration metadata")?;

    if migrations_table_exists == 0 {
        anyhow::bail!(
            "Tasker database schema is not initialized. Run `tasker init` for a new Task Backend, or run `tasker db migrate` from the Managed Source Repository Main Branch for an existing Task Backend."
        );
    }

    if let Some(version) = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM _sqlx_migrations WHERE success = false ORDER BY version LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .context("failed to inspect SQLite dirty migration state")?
    {
        anyhow::bail!(
            "Tasker database migration {version} is marked failed/dirty. Restore from backup or repair the migration state before running Tasker commands."
        );
    }

    let applied = sqlx::query_as::<_, (i64, Vec<u8>)>(
        "SELECT version, checksum FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .context("failed to inspect applied SQLite migrations")?;
    let migrator = migration_source();

    let mut pending = Vec::new();
    for migration in migrator.iter() {
        if migration.migration_type.is_down_migration() {
            continue;
        }
        match applied
            .iter()
            .find(|(version, _)| *version == migration.version)
        {
            Some((_, checksum)) if checksum.as_slice() != migration.checksum.as_ref() => {
                anyhow::bail!(friendly_migration_error(
                    sqlx::migrate::MigrateError::VersionMismatch(migration.version,)
                ));
            }
            Some(_) => {}
            None => pending.push(migration.version),
        }
    }

    for (version, _) in &applied {
        if !migrator.version_exists(*version) {
            anyhow::bail!(friendly_migration_error(
                sqlx::migrate::MigrateError::VersionMissing(*version,)
            ));
        }
    }

    Ok(pending)
}

fn migration_source() -> sqlx::migrate::Migrator {
    sqlx::migrate!("./migrations")
}

fn friendly_migration_error(error: sqlx::migrate::MigrateError) -> anyhow::Error {
    match error {
        sqlx::migrate::MigrateError::VersionMissing(version) => anyhow::anyhow!(
            "SQLite migration drift detected: migration {version} was previously applied to the Task Backend but is missing from the resolved migrations in this checkout. Restore the missing migration file, switch to the Managed Source Repository Main Branch that contains it, or intentionally migrate only from Main Branch after the Task Branch is integrated."
        ),
        sqlx::migrate::MigrateError::VersionMismatch(version) => anyhow::anyhow!(
            "SQLite migration drift detected: migration {version} checksum differs from the migration already applied to the Task Backend. Restore the original migration file or repair the database from a trusted Main Branch checkout."
        ),
        sqlx::migrate::MigrateError::Dirty(version) => anyhow::anyhow!(
            "Tasker database migration {version} is marked failed/dirty. Restore from backup or repair the migration state before running Tasker commands."
        ),
        other => anyhow::anyhow!(other),
    }
}

async fn with_sqlite_write_retry<T, F, Fut>(mut operation: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    for backoff in SQLITE_WRITE_RETRY_BACKOFFS {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) if is_transient_sqlite_write_error(&error) => sleep(backoff).await,
            Err(error) => return Err(error),
        }
    }

    operation().await
}

fn is_transient_sqlite_write_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let Some(sqlx_error) = cause.downcast_ref::<sqlx::Error>() else {
            return false;
        };
        let sqlx::Error::Database(database_error) = sqlx_error else {
            return false;
        };
        let code = database_error.code().unwrap_or_default();
        let message = database_error.message().to_ascii_lowercase();

        matches!(code.as_ref(), "5" | "6" | "SQLITE_BUSY" | "SQLITE_LOCKED")
            || message.contains("database is locked")
            || message.contains("database table is locked")
            || message.contains("database is busy")
    })
}

pub async fn ensure_local_api_token(pool: &SqlitePool) -> Result<String> {
    with_sqlite_write_retry(|| async {
        if let Some(token) = get_api_token(pool, LOCAL_TOKEN_NAME).await? {
            return Ok(token);
        }

        let token = format!("tasker_{}", Uuid::new_v4().simple());
        sqlx::query(
            r#"
            INSERT INTO api_tokens (id, name, token)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(LOCAL_TOKEN_NAME)
        .bind(&token)
        .execute(pool)
        .await
        .context("failed to create local API token")?;

        Ok(token)
    })
    .await
}

pub async fn get_api_token(pool: &SqlitePool, name: &str) -> Result<Option<String>> {
    sqlx::query_scalar("SELECT token FROM api_tokens WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("failed to load API token {name}"))
}

pub async fn authenticate_api_token(pool: &SqlitePool, token: &str) -> Result<bool> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_tokens WHERE token = ?")
        .bind(token)
        .fetch_one(pool)
        .await
        .context("failed to authenticate API token")?;
    Ok(count > 0)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Actor {
    pub kind: String,
    pub id: String,
    pub display_name: String,
}

impl Actor {
    pub fn operator(display_name: impl Into<String>) -> Self {
        let display_name = display_name.into();
        Self {
            kind: "operator".to_string(),
            id: display_name.clone(),
            display_name,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateTaskQueue {
    pub key: String,
    pub name: String,
    pub managed_source_repository: String,
    pub main_branch: String,
    pub worktree_root: String,
    pub branch_template: String,
    pub done_worktree_retention: bool,
    pub queue_concurrency_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskQueue {
    pub id: String,
    pub key: String,
    pub name: String,
    pub delivery_backend: String,
    pub managed_source_repository: String,
    pub main_branch: String,
    pub worktree_root: String,
    pub branch_template: String,
    pub done_worktree_retention: bool,
    pub queue_concurrency_limit: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateQueueConcurrencyLimit {
    pub queue_concurrency_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct AuditEvent {
    pub id: String,
    pub actor_kind: String,
    pub actor_id: String,
    pub actor_display_name: String,
    pub event_type: String,
    pub subject_type: String,
    pub subject_id: String,
    pub payload_json: String,
    pub created_at: String,
}

pub async fn create_task_queue(
    pool: &SqlitePool,
    input: &CreateTaskQueue,
    actor: &Actor,
) -> Result<TaskQueue> {
    with_sqlite_write_retry(|| create_task_queue_once(pool, input, actor)).await
}

async fn create_task_queue_once(
    pool: &SqlitePool,
    input: &CreateTaskQueue,
    actor: &Actor,
) -> Result<TaskQueue> {
    validate_actor(actor)?;
    validate_task_queue(input)?;
    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let queue_id = Uuid::new_v4().to_string();
    let audit_id = Uuid::new_v4().to_string();
    let payload_json = serde_json::to_string(input).context("failed to encode audit payload")?;

    sqlx::query(
        r#"
        INSERT INTO task_queues (
            id,
            key,
            name,
            delivery_backend,
            managed_source_repository,
            main_branch,
            worktree_root,
            branch_template,
            done_worktree_retention,
            queue_concurrency_limit
        ) VALUES (?, ?, ?, 'local_worktree', ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&queue_id)
    .bind(&input.key)
    .bind(&input.name)
    .bind(&input.managed_source_repository)
    .bind(&input.main_branch)
    .bind(&input.worktree_root)
    .bind(&input.branch_template)
    .bind(input.done_worktree_retention)
    .bind(input.queue_concurrency_limit)
    .execute(&mut *tx)
    .await
    .with_context(|| format!("failed to create Task Queue {}", input.key))?;

    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id,
            actor_kind,
            actor_id,
            actor_display_name,
            event_type,
            subject_type,
            subject_id,
            payload_json
        ) VALUES (?, ?, ?, ?, 'task_queue.created', 'task_queue', ?, ?)
        "#,
    )
    .bind(&audit_id)
    .bind(&actor.kind)
    .bind(&actor.id)
    .bind(&actor.display_name)
    .bind(&queue_id)
    .bind(payload_json)
    .execute(&mut *tx)
    .await
    .context("failed to append audit event")?;

    tx.commit().await.context("failed to commit transaction")?;

    get_task_queue(pool, &input.key)
        .await?
        .with_context(|| format!("created Task Queue {} was not found", input.key))
}

pub async fn get_task_queue(pool: &SqlitePool, key: &str) -> Result<Option<TaskQueue>> {
    sqlx::query_as::<_, TaskQueue>(
        r#"
        SELECT
            id,
            key,
            name,
            delivery_backend,
            managed_source_repository,
            main_branch,
            worktree_root,
            branch_template,
            done_worktree_retention,
            queue_concurrency_limit,
            created_at,
            updated_at
        FROM task_queues
        WHERE key = ?
        "#,
    )
    .bind(key)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to load Task Queue {key}"))
}

pub async fn list_task_queues(pool: &SqlitePool) -> Result<Vec<TaskQueue>> {
    sqlx::query_as::<_, TaskQueue>(
        r#"
        SELECT
            id,
            key,
            name,
            delivery_backend,
            managed_source_repository,
            main_branch,
            worktree_root,
            branch_template,
            done_worktree_retention,
            queue_concurrency_limit,
            created_at,
            updated_at
        FROM task_queues
        ORDER BY key
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to list Task Queues")
}

pub async fn update_task_queue_concurrency_limit(
    pool: &SqlitePool,
    key: &str,
    input: &UpdateQueueConcurrencyLimit,
    actor: &Actor,
) -> Result<TaskQueue> {
    validate_actor(actor)?;
    validate_queue_concurrency_limit(input.queue_concurrency_limit)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let existing = sqlx::query_as::<_, TaskQueue>(
        r#"
        SELECT
            id, key, name, delivery_backend, managed_source_repository, main_branch,
            worktree_root, branch_template, done_worktree_retention, queue_concurrency_limit,
            created_at, updated_at
        FROM task_queues
        WHERE key = ?
        "#,
    )
    .bind(key)
    .fetch_optional(&mut *tx)
    .await
    .with_context(|| format!("failed to load Task Queue {key}"))?
    .with_context(|| format!("Task Queue {key} not found"))?;

    sqlx::query(
        r#"
        UPDATE task_queues
        SET queue_concurrency_limit = ?,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(input.queue_concurrency_limit)
    .bind(&existing.id)
    .execute(&mut *tx)
    .await
    .with_context(|| format!("failed to update Queue Concurrency Limit for Task Queue {key}"))?;

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task_queue.concurrency_limit_updated",
        "task_queue",
        &existing.id,
        serde_json::json!({
            "key": key,
            "previous_queue_concurrency_limit": existing.queue_concurrency_limit,
            "queue_concurrency_limit": input.queue_concurrency_limit,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;

    get_task_queue(pool, key)
        .await?
        .with_context(|| format!("updated Task Queue {key} was not found"))
}

pub async fn list_audit_events(pool: &SqlitePool) -> Result<Vec<AuditEvent>> {
    sqlx::query_as::<_, AuditEvent>(
        r#"
        SELECT
            id,
            actor_kind,
            actor_id,
            actor_display_name,
            event_type,
            subject_type,
            subject_id,
            payload_json,
            created_at
        FROM audit_events
        ORDER BY created_at, id
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to list Audit Events")
}

pub async fn list_task_queue_audit_events(pool: &SqlitePool, key: &str) -> Result<Vec<AuditEvent>> {
    sqlx::query_as::<_, AuditEvent>(
        r#"
        SELECT
            audit_events.id,
            audit_events.actor_kind,
            audit_events.actor_id,
            audit_events.actor_display_name,
            audit_events.event_type,
            audit_events.subject_type,
            audit_events.subject_id,
            audit_events.payload_json,
            audit_events.created_at
        FROM audit_events
        JOIN task_queues ON task_queues.id = audit_events.subject_id
        WHERE audit_events.subject_type = 'task_queue'
          AND task_queues.key = ?
        ORDER BY audit_events.created_at, audit_events.id
        "#,
    )
    .bind(key)
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to list Audit Events for Task Queue {key}"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateTask {
    pub queue_key: String,
    pub title: String,
    pub brief: String,
    pub priority: String,
    pub state: String,
    pub review_required: bool,
    pub acceptance_criteria: Vec<String>,
    pub validation_items: Vec<String>,
    pub tags: Vec<String>,
    #[serde(default)]
    pub conflict_hints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateChildTask {
    pub title: String,
    pub brief: String,
    pub priority: String,
    pub state: String,
    pub review_required: bool,
    pub acceptance_criteria: Vec<String>,
    pub validation_items: Vec<String>,
    pub tags: Vec<String>,
    #[serde(default)]
    pub conflict_hints: Vec<String>,
    pub blocks_parent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub task_queue_id: String,
    pub task_queue_key: String,
    pub identifier: String,
    pub sequence: i64,
    pub title: String,
    pub brief: String,
    pub priority: String,
    pub state: String,
    pub review_required: bool,
    pub validated_base_commit: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct AcceptanceCriterion {
    pub id: String,
    pub task_id: String,
    pub position: i64,
    pub description: String,
    pub status: String,
    pub waiver_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct ValidationItem {
    pub id: String,
    pub task_id: String,
    pub position: i64,
    pub description: String,
    pub status: String,
    pub waiver_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct WorkpadNote {
    pub id: String,
    pub task_id: String,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskLink {
    pub id: String,
    pub task_id: String,
    pub kind: String,
    pub target: String,
    pub label: Option<String>,
    pub is_primary: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskConflictHint {
    pub id: String,
    pub task_id: String,
    pub position: i64,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskConflictOverlap {
    pub target: String,
    pub task_identifier: String,
    pub title: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskConflictGroup {
    pub queue_key: String,
    pub target: String,
    pub task_count: i64,
    pub tasks: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpsertTaskLink {
    pub kind: String,
    pub target: String,
    pub label: Option<String>,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskDetail {
    pub task: Task,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub validation_items: Vec<ValidationItem>,
    pub tags: Vec<String>,
    pub workpad_note: Option<WorkpadNote>,
    pub task_links: Vec<TaskLink>,
    pub conflict_hints: Vec<TaskConflictHint>,
    pub conflict_overlaps: Vec<TaskConflictOverlap>,
    pub latest_rework_reason_code: Option<String>,
    pub latest_rework_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskContextBundle {
    pub task: TaskDetail,
    pub queue: TaskContextQueue,
    pub local_workflow: TaskLocalWorkflowContext,
    pub agent_runs: Vec<TaskContextAgentRun>,
    pub latest_failure: Option<TaskContextRunFailure>,
    pub latest_integration_outcome: Option<TaskContextIntegrationOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskContextQueue {
    pub key: String,
    pub name: String,
    pub delivery_backend: String,
    pub main_branch: String,
    pub managed_source_repository: String,
    pub worktree_root: String,
    pub branch_template: String,
    pub queue_concurrency_limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskLocalWorkflowContext {
    pub local_worktree: Option<String>,
    pub task_branch: Option<String>,
    pub main_branch: String,
    pub managed_source_repository: String,
    pub worktree_root: String,
    pub branch_template: String,
    pub delivery_backend: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskContextAgentRun {
    pub id: String,
    pub worker_actor_kind: String,
    pub worker_actor_id: String,
    pub worker_actor_display_name: String,
    pub worker_id: String,
    pub launcher_kind: String,
    pub lease_expires_at: String,
    pub last_heartbeat_at: Option<String>,
    pub outcome: Option<String>,
    pub failure_reason: Option<String>,
    pub failure_reason_code: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskContextRunFailure {
    pub agent_run_id: String,
    pub outcome: String,
    pub failure_reason: Option<String>,
    pub failure_reason_code: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskContextIntegrationOutcome {
    pub id: String,
    pub agent_run_id: Option<String>,
    pub outcome_kind: String,
    pub reason_code: Option<String>,
    pub final_commit: Option<String>,
    pub pre_merge_head: Option<String>,
    pub message: Option<String>,
    pub retryable: bool,
    pub retry_attempt: Option<i64>,
    pub next_retry_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct QueueStatus {
    pub queue_key: String,
    pub queue_name: String,
    pub queue_concurrency_limit: Option<i64>,
    pub state: String,
    pub task_count: i64,
    pub ready_tasks: i64,
    pub integrating_tasks: i64,
    pub active_agent_runs: i64,
    pub active_integrating_agent_runs: i64,
    pub active_retry_holds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct ActiveAgentRunStatus {
    pub queue_key: String,
    pub task_identifier: String,
    pub task_title: String,
    pub task_state: String,
    pub agent_run_id: String,
    pub launcher_kind: String,
    pub worker_id: String,
    pub lease_expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TaskStatusSummary {
    pub queue_key: String,
    pub identifier: String,
    pub title: String,
    pub state: String,
    pub priority: String,
    pub local_worktree: Option<String>,
    pub task_branch: Option<String>,
    pub main_branch: String,
    pub latest_rework_reason_code: Option<String>,
    pub latest_rework_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct ActiveRetryHoldStatus {
    pub queue_key: String,
    pub task_identifier: String,
    pub hold_until: String,
    pub reason: String,
    pub failure_reason_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct MergeQueueTask {
    pub queue_key: String,
    pub task_identifier: String,
    pub title: String,
    pub task_branch: Option<String>,
    pub local_worktree: Option<String>,
    pub main_branch: String,
    pub latest_agent_run_id: Option<String>,
    pub latest_agent_run_outcome: Option<String>,
    pub pending_acceptance_criteria: i64,
    pub pending_validation_items: i64,
    pub failed_validation_items: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateRequirementStatus {
    pub status: String,
    pub waiver_reason: Option<String>,
    pub validated_base_commit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransitionTaskState {
    pub to_state: String,
    pub agent_run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct AgentRun {
    pub id: String,
    pub task_id: String,
    pub task_queue_id: String,
    pub worker_actor_kind: String,
    pub worker_actor_id: String,
    pub worker_actor_display_name: String,
    pub worker_id: String,
    pub launcher_kind: String,
    pub lease_expires_at: String,
    pub last_heartbeat_at: Option<String>,
    pub outcome: Option<String>,
    pub failure_reason: Option<String>,
    pub failure_reason_code: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct LauncherSessionData {
    pub agent_run_id: String,
    pub launcher_kind: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub final_status: Option<String>,
    pub transcript_path: Option<String>,
    pub raw_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct AgentRunMetrics {
    pub agent_run_id: String,
    pub duration_ms: Option<i64>,
    pub launcher_kind: String,
    pub final_status: Option<String>,
    pub exit_code: Option<i64>,
    pub timed_out: Option<i64>,
    pub unattended_question_detected: Option<i64>,
    pub blocking_ui_detected: Option<i64>,
    pub transcript_path: Option<String>,
    pub transcript_byte_size: Option<i64>,
    pub transcript_jsonl_event_count: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub tool_call_count: Option<i64>,
    pub tool_error_count: Option<i64>,
    pub repeated_failed_tool_attempt_count: Option<i64>,
    pub tool_call_counts_json: String,
    pub repeated_read_count: Option<i64>,
    pub repeated_tasker_context_fetch_count: Option<i64>,
    pub shell_command_counts_json: String,
    pub assistant_turn_count: Option<i64>,
    pub user_turn_count: Option<i64>,
    pub max_context_tokens: Option<i64>,
    pub efficiency_hints_json: String,
    pub warnings_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComputedAgentRunMetrics {
    pub agent_run_id: String,
    pub duration_ms: Option<i64>,
    pub launcher_kind: String,
    pub final_status: Option<String>,
    pub exit_code: Option<i64>,
    pub timed_out: Option<i64>,
    pub unattended_question_detected: Option<i64>,
    pub blocking_ui_detected: Option<i64>,
    pub transcript_path: Option<String>,
    pub transcript_byte_size: Option<i64>,
    pub transcript_jsonl_event_count: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub tool_call_count: Option<i64>,
    pub tool_error_count: Option<i64>,
    pub repeated_failed_tool_attempt_count: Option<i64>,
    pub tool_call_counts_json: String,
    pub repeated_read_count: Option<i64>,
    pub repeated_tasker_context_fetch_count: Option<i64>,
    pub shell_command_counts_json: String,
    pub assistant_turn_count: Option<i64>,
    pub user_turn_count: Option<i64>,
    pub max_context_tokens: Option<i64>,
    pub efficiency_hints_json: String,
    pub warnings_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpsertLauncherSessionData {
    pub launcher_kind: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub final_status: Option<String>,
    pub transcript_path: Option<String>,
    pub raw_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunDetail {
    pub run: AgentRun,
    pub task: TaskDetail,
    pub launcher_session_data: Option<LauncherSessionData>,
    pub metrics: Option<AgentRunMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimedRun {
    pub run: AgentRun,
    pub task: TaskDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimNextInput {
    pub queue_key: String,
    pub worker_id: String,
    pub launcher_kind: String,
    pub lease_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinishRunInput {
    pub outcome: String,
    pub failure_reason: Option<String>,
    pub failure_reason_code: Option<String>,
    pub retry_hold_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorFailRunInput {
    pub failure_reason: String,
    pub failure_reason_code: Option<String>,
    pub retry_hold_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct IntegrationOutcome {
    pub id: String,
    pub task_id: String,
    pub agent_run_id: Option<String>,
    pub outcome_kind: String,
    pub reason_code: Option<String>,
    pub final_commit: Option<String>,
    pub pre_merge_head: Option<String>,
    pub message: Option<String>,
    pub retryable: bool,
    pub retry_attempt: Option<i64>,
    pub next_retry_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordIntegrationOutcomeInput {
    pub task_identifier: String,
    pub agent_run_id: Option<String>,
    pub outcome_kind: String,
    pub reason_code: String,
    pub final_commit: Option<String>,
    pub pre_merge_head: Option<String>,
    pub message: Option<String>,
    pub retryable: bool,
    pub retry_attempt: Option<i64>,
    pub retry_delay_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct IntegrationRetryStatus {
    pub queue_key: String,
    pub task_identifier: String,
    pub task_title: String,
    pub reason_code: String,
    pub retryable: bool,
    pub retry_attempt: Option<i64>,
    pub next_retry_at: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetryTaskInput {
    pub reason: String,
}

pub async fn create_task(
    pool: &SqlitePool,
    input: &CreateTask,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_actor(actor)?;
    validate_create_task(input)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let queue = sqlx::query_as::<_, TaskQueue>(
        r#"
        SELECT id, key, name, delivery_backend, managed_source_repository, main_branch,
               worktree_root, branch_template, done_worktree_retention, queue_concurrency_limit, created_at, updated_at
        FROM task_queues
        WHERE key = ?
        "#,
    )
    .bind(&input.queue_key)
    .fetch_optional(&mut *tx)
    .await
    .with_context(|| format!("failed to load Task Queue {}", input.queue_key))?
    .with_context(|| format!("Task Queue {} not found", input.queue_key))?;

    let sequence: i64 = sqlx::query_scalar(
        r#"
        UPDATE task_queues
        SET next_task_sequence = next_task_sequence + 1,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        RETURNING next_task_sequence - 1
        "#,
    )
    .bind(&queue.id)
    .fetch_one(&mut *tx)
    .await
    .context("failed to allocate Task Identifier sequence")?;

    let task_id = Uuid::new_v4().to_string();
    let identifier = format!("{}-{}", queue.key, sequence);
    sqlx::query(
        r#"
        INSERT INTO tasks (
            id, task_queue_id, identifier, sequence, title, brief, priority, state, review_required
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&task_id)
    .bind(&queue.id)
    .bind(&identifier)
    .bind(sequence)
    .bind(&input.title)
    .bind(&input.brief)
    .bind(&input.priority)
    .bind(&input.state)
    .bind(input.review_required)
    .execute(&mut *tx)
    .await
    .with_context(|| format!("failed to create Task {identifier}"))?;

    for (index, description) in input.acceptance_criteria.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO acceptance_criteria (id, task_id, position, description)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&task_id)
        .bind((index + 1) as i64)
        .bind(description)
        .execute(&mut *tx)
        .await
        .context("failed to create Acceptance Criterion")?;
    }

    for (index, description) in input.validation_items.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO validation_items (id, task_id, position, description)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&task_id)
        .bind((index + 1) as i64)
        .bind(description)
        .execute(&mut *tx)
        .await
        .context("failed to create Validation Item")?;
    }

    for tag in normalized_tags(&input.tags) {
        sqlx::query("INSERT INTO task_tags (task_id, tag) VALUES (?, ?)")
            .bind(&task_id)
            .bind(tag)
            .execute(&mut *tx)
            .await
            .context("failed to create Task Tag")?;
    }

    let conflict_hints = normalized_conflict_hints(&input.conflict_hints);
    for (index, target) in conflict_hints.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO task_conflict_hints (id, task_id, position, target)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&task_id)
        .bind((index + 1) as i64)
        .bind(target)
        .execute(&mut *tx)
        .await
        .context("failed to create Task Conflict Hint")?;
    }

    let payload_json = serde_json::json!({
        "identifier": identifier,
        "queue_key": queue.key,
        "title": input.title,
        "priority": input.priority,
        "state": input.state,
        "review_required": input.review_required,
        "acceptance_criteria_count": input.acceptance_criteria.len(),
        "validation_items_count": input.validation_items.len(),
        "tags": normalized_tags(&input.tags),
        "conflict_hints": conflict_hints,
    })
    .to_string();
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_kind, actor_id, actor_display_name, event_type, subject_type, subject_id, payload_json
        ) VALUES (?, ?, ?, ?, 'task.created', 'task', ?, ?)
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&actor.kind)
    .bind(&actor.id)
    .bind(&actor.display_name)
    .bind(&task_id)
    .bind(payload_json)
    .execute(&mut *tx)
    .await
    .context("failed to append audit event")?;

    tx.commit().await.context("failed to commit transaction")?;

    get_task_detail(pool, &identifier)
        .await?
        .with_context(|| format!("created Task {identifier} was not found"))
}

pub async fn create_child_task(
    pool: &SqlitePool,
    parent_identifier: &str,
    input: &CreateChildTask,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_child_task_actor(actor)?;
    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let parent = sqlx::query_as::<_, Task>(
        r#"
        SELECT tasks.id, tasks.task_queue_id, task_queues.key AS task_queue_key, tasks.identifier,
               tasks.sequence, tasks.title, tasks.brief, tasks.priority, tasks.state,
               tasks.review_required, tasks.validated_base_commit, tasks.created_at, tasks.updated_at
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.identifier = ?
        "#,
    )
    .bind(parent_identifier)
    .fetch_optional(&mut *tx)
    .await
    .with_context(|| format!("failed to load Task {parent_identifier}"))?
    .with_context(|| format!("Task {parent_identifier} not found"))?;
    let child_input = CreateTask {
        queue_key: parent.task_queue_key.clone(),
        title: input.title.clone(),
        brief: input.brief.clone(),
        priority: input.priority.clone(),
        state: input.state.clone(),
        review_required: input.review_required,
        acceptance_criteria: input.acceptance_criteria.clone(),
        validation_items: input.validation_items.clone(),
        tags: input.tags.clone(),
        conflict_hints: input.conflict_hints.clone(),
    };
    validate_create_task(&child_input)?;

    let sequence: i64 = sqlx::query_scalar(
        r#"
        UPDATE task_queues
        SET next_task_sequence = next_task_sequence + 1,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        RETURNING next_task_sequence - 1
        "#,
    )
    .bind(&parent.task_queue_id)
    .fetch_one(&mut *tx)
    .await
    .context("failed to allocate Child Task Identifier sequence")?;
    let child_task_id = Uuid::new_v4().to_string();
    let child_identifier = format!("{}-{}", parent.task_queue_key, sequence);
    sqlx::query(
        r#"
        INSERT INTO tasks (id, task_queue_id, identifier, sequence, title, brief, priority, state, review_required)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&child_task_id)
    .bind(&parent.task_queue_id)
    .bind(&child_identifier)
    .bind(sequence)
    .bind(&child_input.title)
    .bind(&child_input.brief)
    .bind(&child_input.priority)
    .bind(&child_input.state)
    .bind(child_input.review_required)
    .execute(&mut *tx)
    .await
    .with_context(|| format!("failed to create Child Task {child_identifier}"))?;
    for (index, description) in child_input.acceptance_criteria.iter().enumerate() {
        sqlx::query("INSERT INTO acceptance_criteria (id, task_id, position, description) VALUES (?, ?, ?, ?)")
            .bind(Uuid::new_v4().to_string())
            .bind(&child_task_id)
            .bind((index + 1) as i64)
            .bind(description)
            .execute(&mut *tx)
            .await
            .context("failed to create Child Task Acceptance Criterion")?;
    }
    for (index, description) in child_input.validation_items.iter().enumerate() {
        sqlx::query(
            "INSERT INTO validation_items (id, task_id, position, description) VALUES (?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&child_task_id)
        .bind((index + 1) as i64)
        .bind(description)
        .execute(&mut *tx)
        .await
        .context("failed to create Child Task Validation Item")?;
    }
    for tag in normalized_tags(&child_input.tags) {
        sqlx::query("INSERT INTO task_tags (task_id, tag) VALUES (?, ?)")
            .bind(&child_task_id)
            .bind(tag)
            .execute(&mut *tx)
            .await
            .context("failed to create Child Task Tag")?;
    }
    let child_conflict_hints = normalized_conflict_hints(&child_input.conflict_hints);
    for (index, target) in child_conflict_hints.iter().enumerate() {
        sqlx::query(
            "INSERT INTO task_conflict_hints (id, task_id, position, target) VALUES (?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&child_task_id)
        .bind((index + 1) as i64)
        .bind(target)
        .execute(&mut *tx)
        .await
        .context("failed to create Child Task Conflict Hint")?;
    }
    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task.created",
        "task",
        &child_task_id,
        serde_json::json!({
            "identifier": child_identifier,
            "queue_key": parent.task_queue_key,
            "title": child_input.title,
            "priority": child_input.priority,
            "state": child_input.state,
            "review_required": child_input.review_required,
            "acceptance_criteria_count": child_input.acceptance_criteria.len(),
            "validation_items_count": child_input.validation_items.len(),
            "tags": normalized_tags(&child_input.tags),
            "conflict_hints": child_conflict_hints,
        }),
    )
    .await?;
    sqlx::query("INSERT INTO task_relationships (id, source_task_id, target_task_id, relationship_kind) VALUES (?, ?, ?, 'parent_child')")
        .bind(Uuid::new_v4().to_string())
        .bind(&parent.id)
        .bind(&child_task_id)
        .execute(&mut *tx)
        .await
        .context("failed to create Child Task relationship")?;
    if input.blocks_parent {
        sqlx::query("INSERT INTO task_relationships (id, source_task_id, target_task_id, relationship_kind) VALUES (?, ?, ?, 'blocks')")
            .bind(Uuid::new_v4().to_string())
            .bind(&child_task_id)
            .bind(&parent.id)
            .execute(&mut *tx)
            .await
            .context("failed to create Blocking Task relationship")?;
    }
    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task.child_created",
        "task",
        &parent.id,
        serde_json::json!({
            "parent_identifier": parent_identifier,
            "child_identifier": child_identifier,
            "blocks_parent": input.blocks_parent,
        }),
    )
    .await?;
    tx.commit().await.context("failed to commit transaction")?;

    get_task_detail(pool, &child_identifier)
        .await?
        .with_context(|| format!("created Child Task {child_identifier} was not found"))
}

pub async fn get_task_detail(pool: &SqlitePool, identifier: &str) -> Result<Option<TaskDetail>> {
    let Some(task) = sqlx::query_as::<_, Task>(
        r#"
        SELECT
            tasks.id,
            tasks.task_queue_id,
            task_queues.key AS task_queue_key,
            tasks.identifier,
            tasks.sequence,
            tasks.title,
            tasks.brief,
            tasks.priority,
            tasks.state,
            tasks.review_required,
            tasks.validated_base_commit,
            tasks.created_at,
            tasks.updated_at
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.identifier = ?
        "#,
    )
    .bind(identifier)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to load Task {identifier}"))?
    else {
        return Ok(None);
    };

    let acceptance_criteria = sqlx::query_as::<_, AcceptanceCriterion>(
        r#"
        SELECT id, task_id, position, description, status, waiver_reason
        FROM acceptance_criteria
        WHERE task_id = ?
        ORDER BY position
        "#,
    )
    .bind(&task.id)
    .fetch_all(pool)
    .await
    .context("failed to load Acceptance Criteria")?;

    let validation_items = sqlx::query_as::<_, ValidationItem>(
        r#"
        SELECT id, task_id, position, description, status, waiver_reason
        FROM validation_items
        WHERE task_id = ?
        ORDER BY position
        "#,
    )
    .bind(&task.id)
    .fetch_all(pool)
    .await
    .context("failed to load Validation Items")?;

    let tags = sqlx::query_scalar(
        r#"
        SELECT tag
        FROM task_tags
        WHERE task_id = ?
        ORDER BY tag
        "#,
    )
    .bind(&task.id)
    .fetch_all(pool)
    .await
    .context("failed to load Task Tags")?;

    let workpad_note = sqlx::query_as::<_, WorkpadNote>(
        r#"
        SELECT id, task_id, body, created_at, updated_at
        FROM workpad_notes
        WHERE task_id = ?
        "#,
    )
    .bind(&task.id)
    .fetch_optional(pool)
    .await
    .context("failed to load Workpad Note")?;

    let task_links = sqlx::query_as::<_, TaskLink>(
        r#"
        SELECT id, task_id, kind, target, label, is_primary, created_at, updated_at
        FROM task_links
        WHERE task_id = ?
        ORDER BY is_primary DESC, kind, target
        "#,
    )
    .bind(&task.id)
    .fetch_all(pool)
    .await
    .context("failed to load Task Links")?;

    let conflict_hints = sqlx::query_as::<_, TaskConflictHint>(
        r#"
        SELECT id, task_id, position, target
        FROM task_conflict_hints
        WHERE task_id = ?
        ORDER BY position
        "#,
    )
    .bind(&task.id)
    .fetch_all(pool)
    .await
    .context("failed to load Task Conflict Hints")?;

    let conflict_overlaps = sqlx::query_as::<_, TaskConflictOverlap>(
        r#"
        SELECT
            self_hints.target AS target,
            other_tasks.identifier AS task_identifier,
            other_tasks.title AS title,
            other_tasks.state AS state
        FROM task_conflict_hints AS self_hints
        JOIN task_conflict_hints AS other_hints
          ON other_hints.target = self_hints.target
         AND other_hints.task_id != self_hints.task_id
        JOIN tasks AS other_tasks ON other_tasks.id = other_hints.task_id
        WHERE self_hints.task_id = ?
          AND other_tasks.task_queue_id = ?
          AND other_tasks.state IN ('ready', 'in_progress')
        ORDER BY self_hints.position, other_tasks.identifier
        "#,
    )
    .bind(&task.id)
    .bind(&task.task_queue_id)
    .fetch_all(pool)
    .await
    .context("failed to load Task Conflict overlaps")?;

    let latest_rework_outcome = sqlx::query_as::<_, TaskContextIntegrationOutcome>(
        r#"
        SELECT
            id,
            agent_run_id,
            outcome_kind,
            reason_code,
            final_commit,
            pre_merge_head,
            message,
            retryable,
            retry_attempt,
            next_retry_at,
            created_at
        FROM integration_outcomes
        WHERE task_id = ?
          AND outcome_kind != 'success'
        ORDER BY created_at DESC, rowid DESC
        LIMIT 1
        "#,
    )
    .bind(&task.id)
    .fetch_optional(pool)
    .await
    .context("failed to load latest Rework Integration Outcome")?;
    let latest_rework_run = if latest_rework_outcome.is_none() {
        sqlx::query_as::<_, TaskContextRunFailure>(
            r#"
            SELECT
                id AS agent_run_id,
                outcome AS outcome,
                failure_reason,
                failure_reason_code,
                finished_at
            FROM agent_runs
            WHERE task_id = ?
              AND outcome IS NOT NULL
              AND outcome != 'completed'
            ORDER BY finished_at DESC, created_at DESC, id DESC
            LIMIT 1
            "#,
        )
        .bind(&task.id)
        .fetch_optional(pool)
        .await
        .context("failed to load latest Rework Agent Run failure")?
    } else {
        None
    };
    let latest_rework_reason_code = latest_rework_outcome
        .as_ref()
        .and_then(|outcome| outcome.reason_code.clone())
        .or_else(|| {
            latest_rework_run
                .as_ref()
                .and_then(|run| run.failure_reason_code.clone())
        });
    let latest_rework_reason = latest_rework_outcome
        .as_ref()
        .and_then(|outcome| outcome.message.clone())
        .or_else(|| {
            latest_rework_run
                .as_ref()
                .and_then(|run| run.failure_reason.clone())
        });

    Ok(Some(TaskDetail {
        task,
        acceptance_criteria,
        validation_items,
        tags,
        workpad_note,
        task_links,
        conflict_hints,
        conflict_overlaps,
        latest_rework_reason_code,
        latest_rework_reason,
    }))
}

pub async fn get_task_context_bundle(
    pool: &SqlitePool,
    identifier: &str,
) -> Result<Option<TaskContextBundle>> {
    let Some(task) = get_task_detail(pool, identifier).await? else {
        return Ok(None);
    };
    let queue = get_task_queue(pool, &task.task.task_queue_key)
        .await?
        .with_context(|| format!("Task Queue {} not found", task.task.task_queue_key))?;
    let local_worktree = primary_task_link_target(&task.task_links, "local_worktree");
    let task_branch = primary_task_link_target(&task.task_links, "task_branch");

    let agent_runs = sqlx::query_as::<_, TaskContextAgentRun>(
        r#"
        SELECT
            id,
            worker_actor_kind,
            worker_actor_id,
            worker_actor_display_name,
            worker_id,
            launcher_kind,
            lease_expires_at,
            last_heartbeat_at,
            outcome,
            failure_reason,
            failure_reason_code,
            created_at,
            finished_at,
            outcome IS NULL AND lease_expires_at > CURRENT_TIMESTAMP AS is_active
        FROM agent_runs
        WHERE task_id = ?
        ORDER BY created_at DESC, id DESC
        LIMIT 5
        "#,
    )
    .bind(&task.task.id)
    .fetch_all(pool)
    .await
    .context("failed to load recent Agent Runs for Task context bundle")?;

    let latest_failure = sqlx::query_as::<_, TaskContextRunFailure>(
        r#"
        SELECT
            id AS agent_run_id,
            outcome AS outcome,
            failure_reason,
            failure_reason_code,
            finished_at
        FROM agent_runs
        WHERE task_id = ?
          AND outcome IS NOT NULL
          AND outcome != 'completed'
        ORDER BY finished_at DESC, created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(&task.task.id)
    .fetch_optional(pool)
    .await
    .context("failed to load latest Agent Run failure for Task context bundle")?;

    let latest_integration_outcome = sqlx::query_as::<_, TaskContextIntegrationOutcome>(
        r#"
        SELECT
            id,
            agent_run_id,
            outcome_kind,
            reason_code,
            final_commit,
            pre_merge_head,
            message,
            retryable,
            retry_attempt,
            next_retry_at,
            created_at
        FROM integration_outcomes
        WHERE task_id = ?
        ORDER BY created_at DESC, rowid DESC
        LIMIT 1
        "#,
    )
    .bind(&task.task.id)
    .fetch_optional(pool)
    .await
    .context("failed to load latest Integration Outcome for Task context bundle")?;

    Ok(Some(TaskContextBundle {
        task,
        queue: TaskContextQueue {
            key: queue.key,
            name: queue.name,
            delivery_backend: queue.delivery_backend.clone(),
            main_branch: queue.main_branch.clone(),
            managed_source_repository: queue.managed_source_repository.clone(),
            worktree_root: queue.worktree_root.clone(),
            branch_template: queue.branch_template.clone(),
            queue_concurrency_limit: queue.queue_concurrency_limit,
        },
        local_workflow: TaskLocalWorkflowContext {
            local_worktree,
            task_branch,
            main_branch: queue.main_branch,
            managed_source_repository: queue.managed_source_repository,
            worktree_root: queue.worktree_root,
            branch_template: queue.branch_template,
            delivery_backend: queue.delivery_backend,
        },
        agent_runs,
        latest_failure,
        latest_integration_outcome,
    }))
}

fn primary_task_link_target(task_links: &[TaskLink], kind: &str) -> Option<String> {
    task_links
        .iter()
        .find(|link| link.kind == kind && link.is_primary)
        .or_else(|| task_links.iter().find(|link| link.kind == kind))
        .map(|link| link.target.clone())
}

pub async fn upsert_task_link(
    pool: &SqlitePool,
    identifier: &str,
    input: &UpsertTaskLink,
    actor: &Actor,
) -> Result<TaskDetail> {
    with_sqlite_write_retry(|| upsert_task_link_once(pool, identifier, input, actor)).await
}

async fn upsert_task_link_once(
    pool: &SqlitePool,
    identifier: &str,
    input: &UpsertTaskLink,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_actor(actor)?;
    ensure_not_blank("Task Link kind", &input.kind)?;
    ensure_not_blank("Task Link target", &input.target)?;
    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let task_id: String = sqlx::query_scalar("SELECT id FROM tasks WHERE identifier = ?")
        .bind(identifier)
        .fetch_optional(&mut *tx)
        .await
        .with_context(|| format!("failed to load Task {identifier}"))?
        .with_context(|| format!("Task {identifier} not found"))?;

    if input.is_primary {
        sqlx::query("UPDATE task_links SET is_primary = 0, updated_at = CURRENT_TIMESTAMP WHERE task_id = ?")
            .bind(&task_id)
            .execute(&mut *tx)
            .await
            .context("failed to clear Primary Handoff Link")?;
    }

    let link_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO task_links (id, task_id, kind, target, label, is_primary)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(task_id, kind, target) DO UPDATE SET
            label = excluded.label,
            is_primary = excluded.is_primary,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(&link_id)
    .bind(&task_id)
    .bind(&input.kind)
    .bind(&input.target)
    .bind(&input.label)
    .bind(input.is_primary)
    .execute(&mut *tx)
    .await
    .context("failed to upsert Task Link")?;

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task_link.upserted",
        "task",
        &task_id,
        serde_json::json!({
            "identifier": identifier,
            "kind": input.kind,
            "target": input.target,
            "label": input.label,
            "is_primary": input.is_primary,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;
    get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("updated Task {identifier} was not found"))
}

pub async fn update_workpad_note(
    pool: &SqlitePool,
    identifier: &str,
    body: &str,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_actor(actor)?;
    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let task_id: String = sqlx::query_scalar("SELECT id FROM tasks WHERE identifier = ?")
        .bind(identifier)
        .fetch_optional(&mut *tx)
        .await
        .with_context(|| format!("failed to load Task {identifier}"))?
        .with_context(|| format!("Task {identifier} not found"))?;

    let existing = sqlx::query_as::<_, WorkpadNote>(
        "SELECT id, task_id, body, created_at, updated_at FROM workpad_notes WHERE task_id = ?",
    )
    .bind(&task_id)
    .fetch_optional(&mut *tx)
    .await
    .context("failed to load Workpad Note")?;

    let workpad_note_id = if let Some(note) = existing {
        sqlx::query("INSERT INTO workpad_revisions (id, workpad_note_id, body) VALUES (?, ?, ?)")
            .bind(Uuid::new_v4().to_string())
            .bind(&note.id)
            .bind(&note.body)
            .execute(&mut *tx)
            .await
            .context("failed to create Workpad Revision")?;
        sqlx::query(
            "UPDATE workpad_notes SET body = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(body)
        .bind(&note.id)
        .execute(&mut *tx)
        .await
        .context("failed to update Workpad Note")?;
        note.id
    } else {
        let note_id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO workpad_notes (id, task_id, body) VALUES (?, ?, ?)")
            .bind(&note_id)
            .bind(&task_id)
            .bind(body)
            .execute(&mut *tx)
            .await
            .context("failed to create Workpad Note")?;
        note_id
    };

    let payload_json = serde_json::json!({
        "identifier": identifier,
        "workpad_note_id": workpad_note_id,
    })
    .to_string();
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_kind, actor_id, actor_display_name, event_type, subject_type, subject_id, payload_json
        ) VALUES (?, ?, ?, ?, 'workpad_note.updated', 'task', ?, ?)
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&actor.kind)
    .bind(&actor.id)
    .bind(&actor.display_name)
    .bind(&task_id)
    .bind(payload_json)
    .execute(&mut *tx)
    .await
    .context("failed to append audit event")?;

    tx.commit().await.context("failed to commit transaction")?;

    get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("updated Task {identifier} was not found"))
}

pub async fn transition_task_state(
    pool: &SqlitePool,
    identifier: &str,
    input: &TransitionTaskState,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_actor(actor)?;
    validate_state(&input.to_state)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let task = sqlx::query_as::<_, Task>(
        r#"
        SELECT
            tasks.id,
            tasks.task_queue_id,
            task_queues.key AS task_queue_key,
            tasks.identifier,
            tasks.sequence,
            tasks.title,
            tasks.brief,
            tasks.priority,
            tasks.state,
            tasks.review_required,
            tasks.validated_base_commit,
            tasks.created_at,
            tasks.updated_at
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.identifier = ?
        "#,
    )
    .bind(identifier)
    .fetch_optional(&mut *tx)
    .await
    .with_context(|| format!("failed to load Task {identifier}"))?
    .with_context(|| format!("Task {identifier} not found"))?;

    validate_transition(&task, &input.to_state, actor)?;
    if input.to_state == "ready" {
        ensure_ready_requirements_exist(&mut tx, &task.id).await?;
    }
    if requires_completion_gates(&input.to_state) {
        ensure_completion_gates_pass(&mut tx, &task.id).await?;
    }
    if actor.kind == "worker_agent" {
        ensure_worker_owns_active_run(&mut tx, &task.id, input.agent_run_id.as_deref(), actor)
            .await?;
    }

    let update = sqlx::query(
        "UPDATE tasks SET state = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ? AND state = ?",
    )
    .bind(&input.to_state)
    .bind(&task.id)
    .bind(&task.state)
    .execute(&mut *tx)
    .await
    .context("failed to transition Task State")?;
    if update.rows_affected() != 1 {
        anyhow::bail!("Task State changed while attempting State Transition");
    }
    let deleted_holds = sqlx::query("DELETE FROM task_retry_holds WHERE task_id = ?")
        .bind(&task.id)
        .execute(&mut *tx)
        .await
        .context("failed to clear Retry Hold after State Transition")?;
    if deleted_holds.rows_affected() > 0 {
        append_audit_event_in_tx(
            &mut tx,
            actor,
            "task.retry_hold_cleared",
            "task",
            &task.id,
            serde_json::json!({ "identifier": identifier, "reason": "Task State changed" }),
        )
        .await?;
    }
    if input.to_state == "canceled" {
        let canceled_runs = sqlx::query(
            r#"
            UPDATE agent_runs
            SET outcome = 'canceled', finished_at = CURRENT_TIMESTAMP, failure_reason = 'Task canceled', failure_reason_code = 'task_canceled'
            WHERE task_id = ? AND outcome IS NULL
            "#,
        )
        .bind(&task.id)
        .execute(&mut *tx)
        .await
        .context("failed to cancel active Agent Runs")?;
        if canceled_runs.rows_affected() > 0 {
            append_audit_event_in_tx(
                &mut tx,
                actor,
                "agent_run.canceled_for_task",
                "task",
                &task.id,
                serde_json::json!({
                    "identifier": identifier,
                    "canceled_runs": canceled_runs.rows_affected(),
                }),
            )
            .await?;
        }
    }
    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task.state_transitioned",
        "task",
        &task.id,
        serde_json::json!({
            "identifier": identifier,
            "from": task.state,
            "to": input.to_state,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;
    get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("transitioned Task {identifier} was not found"))
}

pub async fn update_acceptance_criterion_status(
    pool: &SqlitePool,
    identifier: &str,
    position: i64,
    input: &UpdateRequirementStatus,
    actor: &Actor,
) -> Result<TaskDetail> {
    update_requirement_status(
        pool,
        identifier,
        position,
        input,
        actor,
        RequirementKind {
            table: "acceptance_criteria",
            event_type: "acceptance_criterion.status_updated",
            allowed_statuses: &["pending", "satisfied", "waived"],
        },
    )
    .await
}

pub async fn update_validation_item_status(
    pool: &SqlitePool,
    identifier: &str,
    position: i64,
    input: &UpdateRequirementStatus,
    actor: &Actor,
) -> Result<TaskDetail> {
    update_requirement_status(
        pool,
        identifier,
        position,
        input,
        actor,
        RequirementKind {
            table: "validation_items",
            event_type: "validation_item.status_updated",
            allowed_statuses: &["pending", "passed", "failed", "waived"],
        },
    )
    .await
}

struct RequirementKind {
    table: &'static str,
    event_type: &'static str,
    allowed_statuses: &'static [&'static str],
}

async fn update_requirement_status(
    pool: &SqlitePool,
    identifier: &str,
    position: i64,
    input: &UpdateRequirementStatus,
    actor: &Actor,
    kind: RequirementKind,
) -> Result<TaskDetail> {
    validate_actor(actor)?;
    validate_requirement_status(input, actor, kind.allowed_statuses)?;
    if position < 1 {
        anyhow::bail!("requirement position must be at least 1");
    }

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let task_id: String = sqlx::query_scalar("SELECT id FROM tasks WHERE identifier = ?")
        .bind(identifier)
        .fetch_optional(&mut *tx)
        .await
        .with_context(|| format!("failed to load Task {identifier}"))?
        .with_context(|| format!("Task {identifier} not found"))?;

    let select_sql = format!(
        "SELECT status FROM {} WHERE task_id = ? AND position = ?",
        kind.table
    );
    let previous_status: String = sqlx::query_scalar(&select_sql)
        .bind(&task_id)
        .bind(position)
        .fetch_optional(&mut *tx)
        .await
        .context("failed to load requirement status")?
        .with_context(|| format!("requirement at position {position} not found"))?;

    let waiver_reason = if input.status == "waived" {
        input
            .waiver_reason
            .as_ref()
            .map(|reason| reason.trim().to_string())
    } else {
        None
    };
    let update_sql = format!(
        "UPDATE {} SET status = ?, waiver_reason = ?, updated_at = CURRENT_TIMESTAMP WHERE task_id = ? AND position = ?",
        kind.table
    );
    sqlx::query(&update_sql)
        .bind(&input.status)
        .bind(&waiver_reason)
        .bind(&task_id)
        .bind(position)
        .execute(&mut *tx)
        .await
        .context("failed to update requirement status")?;

    if kind.table == "validation_items" {
        let validated_base_commit = if input.status == "passed" {
            input
                .validated_base_commit
                .as_deref()
                .map(str::trim)
                .filter(|commit| !commit.is_empty())
        } else {
            None
        };
        sqlx::query("UPDATE tasks SET validated_base_commit = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(validated_base_commit)
            .bind(&task_id)
            .execute(&mut *tx)
            .await
            .context("failed to update Validated Base Commit")?;
    }

    let payload_json = serde_json::json!({
        "identifier": identifier,
        "position": position,
        "previous_status": previous_status,
        "status": input.status,
        "waiver_reason": waiver_reason,
        "validated_base_commit": input.validated_base_commit,
    })
    .to_string();
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_kind, actor_id, actor_display_name, event_type, subject_type, subject_id, payload_json
        ) VALUES (?, ?, ?, ?, ?, 'task', ?, ?)
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&actor.kind)
    .bind(&actor.id)
    .bind(&actor.display_name)
    .bind(kind.event_type)
    .bind(&task_id)
    .bind(payload_json)
    .execute(&mut *tx)
    .await
    .context("failed to append audit event")?;

    tx.commit().await.context("failed to commit transaction")?;

    get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("updated Task {identifier} was not found"))
}

pub async fn list_task_audit_events(
    pool: &SqlitePool,
    identifier: &str,
) -> Result<Vec<AuditEvent>> {
    let task_id: String = sqlx::query_scalar("SELECT id FROM tasks WHERE identifier = ?")
        .bind(identifier)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("failed to load Task {identifier}"))?
        .with_context(|| format!("Task {identifier} not found"))?;

    sqlx::query_as::<_, AuditEvent>(
        r#"
        SELECT id, actor_kind, actor_id, actor_display_name, event_type, subject_type, subject_id, payload_json, created_at
        FROM audit_events
        WHERE subject_type = 'task' AND subject_id = ?
        ORDER BY created_at, id
        "#,
    )
    .bind(task_id)
    .fetch_all(pool)
    .await
    .context("failed to list task Audit Events")
}

pub async fn count_workpad_revisions(pool: &SqlitePool, identifier: &str) -> Result<i64> {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM workpad_revisions
        JOIN workpad_notes ON workpad_notes.id = workpad_revisions.workpad_note_id
        JOIN tasks ON tasks.id = workpad_notes.task_id
        WHERE tasks.identifier = ?
        "#,
    )
    .bind(identifier)
    .fetch_one(pool)
    .await
    .context("failed to count Workpad Revisions")
}

pub async fn status_by_queue_and_state(pool: &SqlitePool) -> Result<Vec<QueueStatus>> {
    sqlx::query_as::<_, QueueStatus>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            task_queues.name AS queue_name,
            task_queues.queue_concurrency_limit AS queue_concurrency_limit,
            COALESCE(tasks.state, 'none') AS state,
            COUNT(tasks.id) AS task_count,
            (
                SELECT COUNT(*) FROM tasks ready_tasks
                WHERE ready_tasks.task_queue_id = task_queues.id
                  AND ready_tasks.state = 'ready'
            ) AS ready_tasks,
            (
                SELECT COUNT(*) FROM tasks integrating_tasks
                WHERE integrating_tasks.task_queue_id = task_queues.id
                  AND integrating_tasks.state = 'integrating'
            ) AS integrating_tasks,
            (
                SELECT COUNT(*) FROM agent_runs
                WHERE agent_runs.task_queue_id = task_queues.id
                  AND agent_runs.outcome IS NULL
                  AND agent_runs.lease_expires_at > CURRENT_TIMESTAMP
            ) AS active_agent_runs,
            (
                SELECT COUNT(*) FROM agent_runs
                JOIN tasks active_tasks ON active_tasks.id = agent_runs.task_id
                WHERE agent_runs.task_queue_id = task_queues.id
                  AND active_tasks.state = 'integrating'
                  AND agent_runs.outcome IS NULL
                  AND agent_runs.lease_expires_at > CURRENT_TIMESTAMP
            ) AS active_integrating_agent_runs,
            (
                SELECT COUNT(*) FROM task_retry_holds
                JOIN tasks held_tasks ON held_tasks.id = task_retry_holds.task_id
                WHERE held_tasks.task_queue_id = task_queues.id
                  AND task_retry_holds.hold_until > CURRENT_TIMESTAMP
            ) AS active_retry_holds
        FROM task_queues
        LEFT JOIN tasks ON tasks.task_queue_id = task_queues.id
        GROUP BY task_queues.id, task_queues.key, task_queues.name, task_queues.queue_concurrency_limit, tasks.state
        ORDER BY task_queues.key, tasks.state
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to load Tasker status")
}

pub async fn active_agent_runs_for_status(pool: &SqlitePool) -> Result<Vec<ActiveAgentRunStatus>> {
    sqlx::query_as::<_, ActiveAgentRunStatus>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS task_identifier,
            tasks.title AS task_title,
            tasks.state AS task_state,
            agent_runs.id AS agent_run_id,
            agent_runs.launcher_kind AS launcher_kind,
            agent_runs.worker_id AS worker_id,
            agent_runs.lease_expires_at AS lease_expires_at
        FROM agent_runs
        JOIN tasks ON tasks.id = agent_runs.task_id
        JOIN task_queues ON task_queues.id = agent_runs.task_queue_id
        WHERE agent_runs.outcome IS NULL
          AND agent_runs.lease_expires_at > CURRENT_TIMESTAMP
        ORDER BY task_queues.key, tasks.identifier, agent_runs.created_at, agent_runs.id
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to load active Agent Runs for status")
}

pub async fn tasks_for_status_by_states(
    pool: &SqlitePool,
    states: &[&str],
) -> Result<Vec<TaskStatusSummary>> {
    if states.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = sqlx::QueryBuilder::new(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS identifier,
            tasks.title AS title,
            tasks.state AS state,
            tasks.priority AS priority,
            task_queues.main_branch AS main_branch,
            (
                SELECT task_links.target FROM task_links
                WHERE task_links.task_id = tasks.id AND task_links.kind = 'local_worktree'
                ORDER BY task_links.is_primary DESC, task_links.created_at DESC, task_links.id DESC
                LIMIT 1
            ) AS local_worktree,
            (
                SELECT task_links.target FROM task_links
                WHERE task_links.task_id = tasks.id AND task_links.kind = 'task_branch'
                ORDER BY task_links.is_primary DESC, task_links.created_at DESC, task_links.id DESC
                LIMIT 1
            ) AS task_branch,
            COALESCE(
                (
                    SELECT integration_outcomes.reason_code FROM integration_outcomes
                    WHERE integration_outcomes.task_id = tasks.id
                      AND integration_outcomes.outcome_kind != 'success'
                    ORDER BY integration_outcomes.created_at DESC, integration_outcomes.rowid DESC
                    LIMIT 1
                ),
                (
                    SELECT agent_runs.failure_reason_code FROM agent_runs
                    WHERE agent_runs.task_id = tasks.id
                      AND agent_runs.outcome IS NOT NULL
                      AND agent_runs.outcome != 'completed'
                    ORDER BY agent_runs.finished_at DESC, agent_runs.created_at DESC, agent_runs.id DESC
                    LIMIT 1
                )
            ) AS latest_rework_reason_code,
            COALESCE(
                (
                    SELECT integration_outcomes.message FROM integration_outcomes
                    WHERE integration_outcomes.task_id = tasks.id
                      AND integration_outcomes.outcome_kind != 'success'
                    ORDER BY integration_outcomes.created_at DESC, integration_outcomes.rowid DESC
                    LIMIT 1
                ),
                (
                    SELECT agent_runs.failure_reason FROM agent_runs
                    WHERE agent_runs.task_id = tasks.id
                      AND agent_runs.outcome IS NOT NULL
                      AND agent_runs.outcome != 'completed'
                    ORDER BY agent_runs.finished_at DESC, agent_runs.created_at DESC, agent_runs.id DESC
                    LIMIT 1
                )
            ) AS latest_rework_reason
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.state IN (
        "#,
    );
    let mut separated = query.separated(", ");
    for state in states {
        separated.push_bind(*state);
    }
    separated.push_unseparated(")");
    query.push(" ORDER BY task_queues.key, tasks.state, tasks.priority, tasks.identifier");
    query
        .build_query_as::<TaskStatusSummary>()
        .fetch_all(pool)
        .await
        .context("failed to load Task summaries for status")
}

pub async fn active_retry_holds_for_status(
    pool: &SqlitePool,
) -> Result<Vec<ActiveRetryHoldStatus>> {
    sqlx::query_as::<_, ActiveRetryHoldStatus>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS task_identifier,
            task_retry_holds.hold_until AS hold_until,
            task_retry_holds.reason AS reason,
            agent_runs.failure_reason_code AS failure_reason_code
        FROM task_retry_holds
        JOIN tasks ON tasks.id = task_retry_holds.task_id
        LEFT JOIN agent_runs ON agent_runs.id = task_retry_holds.agent_run_id
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE task_retry_holds.hold_until > CURRENT_TIMESTAMP
        ORDER BY task_queues.key, tasks.identifier
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to load active Retry Holds for status")
}

pub async fn integration_retries_for_status(
    pool: &SqlitePool,
) -> Result<Vec<IntegrationRetryStatus>> {
    sqlx::query_as::<_, IntegrationRetryStatus>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS task_identifier,
            tasks.title AS task_title,
            COALESCE(latest.reason_code, 'unknown_legacy') AS reason_code,
            latest.retryable AS retryable,
            latest.retry_attempt AS retry_attempt,
            latest.next_retry_at AS next_retry_at,
            latest.message AS reason
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        JOIN integration_outcomes latest ON latest.id = (
            SELECT integration_outcomes.id FROM integration_outcomes
            WHERE integration_outcomes.task_id = tasks.id
            ORDER BY integration_outcomes.created_at DESC, integration_outcomes.rowid DESC
            LIMIT 1
        )
        WHERE tasks.state = 'integrating'
          AND latest.outcome_kind = 'operational_failure'
        ORDER BY task_queues.key, tasks.identifier
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to load Integration retry status")
}

pub async fn due_integration_retries(
    pool: &SqlitePool,
    queue_key: &str,
) -> Result<Vec<IntegrationRetryStatus>> {
    sqlx::query_as::<_, IntegrationRetryStatus>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS task_identifier,
            tasks.title AS task_title,
            COALESCE(latest.reason_code, 'unknown_legacy') AS reason_code,
            latest.retryable AS retryable,
            latest.retry_attempt AS retry_attempt,
            latest.next_retry_at AS next_retry_at,
            latest.message AS reason
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        JOIN integration_outcomes latest ON latest.id = (
            SELECT integration_outcomes.id FROM integration_outcomes
            WHERE integration_outcomes.task_id = tasks.id
            ORDER BY integration_outcomes.created_at DESC, integration_outcomes.rowid DESC
            LIMIT 1
        )
        WHERE task_queues.key = ?
          AND tasks.state = 'integrating'
          AND latest.outcome_kind = 'operational_failure'
          AND latest.retryable = 1
          AND latest.next_retry_at IS NOT NULL
          AND latest.next_retry_at <= CURRENT_TIMESTAMP
          AND NOT EXISTS (
              SELECT 1 FROM agent_runs
              WHERE agent_runs.task_id = tasks.id
                AND agent_runs.outcome IS NULL
                AND agent_runs.lease_expires_at > CURRENT_TIMESTAMP
          )
        ORDER BY latest.next_retry_at, tasks.identifier
        "#,
    )
    .bind(queue_key)
    .fetch_all(pool)
    .await
    .context("failed to load due Integration retries")
}

pub async fn task_conflict_groups_for_status(pool: &SqlitePool) -> Result<Vec<TaskConflictGroup>> {
    sqlx::query_as::<_, TaskConflictGroup>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            task_conflict_hints.target AS target,
            COUNT(DISTINCT tasks.id) AS task_count,
            group_concat(tasks.identifier || ' (' || tasks.state || ')', ', ') AS tasks
        FROM task_conflict_hints
        JOIN tasks ON tasks.id = task_conflict_hints.task_id
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.state IN ('ready', 'in_progress')
        GROUP BY task_queues.key, task_conflict_hints.target
        HAVING COUNT(DISTINCT tasks.id) > 1
        ORDER BY task_queues.key, task_conflict_hints.target
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to load Task conflict hints for status")
}

pub async fn merge_queue_tasks(
    pool: &SqlitePool,
    queue_key: Option<&str>,
) -> Result<Vec<MergeQueueTask>> {
    sqlx::query_as::<_, MergeQueueTask>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS task_identifier,
            tasks.title AS title,
            (
                SELECT task_links.target FROM task_links
                WHERE task_links.task_id = tasks.id AND task_links.kind = 'task_branch'
                ORDER BY task_links.is_primary DESC, task_links.created_at DESC, task_links.id DESC
                LIMIT 1
            ) AS task_branch,
            (
                SELECT task_links.target FROM task_links
                WHERE task_links.task_id = tasks.id AND task_links.kind = 'local_worktree'
                ORDER BY task_links.is_primary DESC, task_links.created_at DESC, task_links.id DESC
                LIMIT 1
            ) AS local_worktree,
            task_queues.main_branch AS main_branch,
            (
                SELECT agent_runs.id FROM agent_runs
                WHERE agent_runs.task_id = tasks.id
                ORDER BY agent_runs.created_at DESC, agent_runs.id DESC
                LIMIT 1
            ) AS latest_agent_run_id,
            (
                SELECT COALESCE(agent_runs.outcome, 'active') FROM agent_runs
                WHERE agent_runs.task_id = tasks.id
                ORDER BY agent_runs.created_at DESC, agent_runs.id DESC
                LIMIT 1
            ) AS latest_agent_run_outcome,
            (
                SELECT COUNT(*) FROM acceptance_criteria
                WHERE acceptance_criteria.task_id = tasks.id
                  AND acceptance_criteria.status NOT IN ('satisfied', 'waived')
            ) AS pending_acceptance_criteria,
            (
                SELECT COUNT(*) FROM validation_items
                WHERE validation_items.task_id = tasks.id
                  AND validation_items.status NOT IN ('passed', 'waived')
            ) AS pending_validation_items,
            (
                SELECT COUNT(*) FROM validation_items
                WHERE validation_items.task_id = tasks.id
                  AND validation_items.status = 'failed'
            ) AS failed_validation_items
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.state = 'integrating'
          AND (? IS NULL OR task_queues.key = ?)
        ORDER BY task_queues.key, tasks.identifier
        "#,
    )
    .bind(queue_key)
    .bind(queue_key)
    .fetch_all(pool)
    .await
    .context("failed to load Manual Dogfood Merge queue")
}

pub async fn claim_next(
    pool: &SqlitePool,
    input: &ClaimNextInput,
    actor: &Actor,
) -> Result<Option<ClaimedRun>> {
    with_sqlite_write_retry(|| claim_next_once(pool, input, actor)).await
}

async fn claim_next_once(
    pool: &SqlitePool,
    input: &ClaimNextInput,
    actor: &Actor,
) -> Result<Option<ClaimedRun>> {
    validate_worker_actor(actor)?;
    ensure_not_blank("worker_id", &input.worker_id)?;
    ensure_not_blank("launcher_kind", &input.launcher_kind)?;
    validate_positive_seconds("lease_seconds", input.lease_seconds)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    expire_stale_agent_runs(&mut tx).await?;

    let queue = sqlx::query_as::<_, TaskQueue>(
        r#"
        SELECT id, key, name, delivery_backend, managed_source_repository, main_branch,
               worktree_root, branch_template, done_worktree_retention, queue_concurrency_limit, created_at, updated_at
        FROM task_queues
        WHERE key = ?
        "#,
    )
    .bind(&input.queue_key)
    .fetch_optional(&mut *tx)
    .await
    .with_context(|| format!("failed to load Task Queue {}", input.queue_key))?
    .with_context(|| format!("Task Queue {} not found", input.queue_key))?;

    if let Some(limit) = queue.queue_concurrency_limit {
        let active_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_runs WHERE task_queue_id = ? AND outcome IS NULL AND lease_expires_at > CURRENT_TIMESTAMP",
        )
        .bind(&queue.id)
        .fetch_one(&mut *tx)
        .await
        .context("failed to count active Agent Runs")?;
        if active_count >= limit {
            tx.commit().await.context("failed to commit transaction")?;
            return Ok(None);
        }
    }

    let claimed_task = sqlx::query_as::<_, Task>(
        r#"
        SELECT tasks.id, tasks.task_queue_id, task_queues.key AS task_queue_key, tasks.identifier,
               tasks.sequence, tasks.title, tasks.brief, tasks.priority, tasks.state,
               tasks.review_required, tasks.validated_base_commit, tasks.created_at, tasks.updated_at
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.task_queue_id = ?
          AND tasks.state IN ('ready', 'in_progress', 'rework')
          AND NOT EXISTS (
              SELECT 1 FROM agent_runs
              WHERE agent_runs.task_id = tasks.id AND agent_runs.outcome IS NULL
          )
          AND NOT EXISTS (
              SELECT 1 FROM task_retry_holds
              WHERE task_retry_holds.task_id = tasks.id AND task_retry_holds.hold_until > CURRENT_TIMESTAMP
          )
        ORDER BY
          CASE tasks.priority
            WHEN 'urgent' THEN 0
            WHEN 'high' THEN 1
            WHEN 'normal' THEN 2
            WHEN 'low' THEN 3
          END,
          tasks.created_at,
          tasks.identifier
        LIMIT 1
        "#,
    )
    .bind(&queue.id)
    .fetch_optional(&mut *tx)
    .await
    .context("failed to claim next Task")?;

    let Some(task) = claimed_task else {
        tx.commit().await.context("failed to commit transaction")?;
        return Ok(None);
    };

    if task.state == "ready" {
        sqlx::query(
            "UPDATE tasks SET state = 'in_progress', updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(&task.id)
        .execute(&mut *tx)
        .await
        .context("failed to move Task to In Progress")?;
        append_audit_event_in_tx(
            &mut tx,
            actor,
            "task.state_changed",
            "task",
            &task.id,
            serde_json::json!({
                "identifier": task.identifier,
                "from": "ready",
                "to": "in_progress",
            }),
        )
        .await?;
    }

    let run_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
                INSERT INTO agent_runs (
                    id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                    worker_actor_display_name, worker_id, launcher_kind, lease_expires_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now', '+' || ? || ' seconds'))
                "#,
    )
    .bind(&run_id)
    .bind(&task.id)
    .bind(&queue.id)
    .bind(&actor.kind)
    .bind(&actor.id)
    .bind(&actor.display_name)
    .bind(&input.worker_id)
    .bind(&input.launcher_kind)
    .bind(input.lease_seconds)
    .execute(&mut *tx)
    .await
    .context("failed to create Agent Run")?;
    let select_run_sql = agent_run_select_sql("WHERE id = ?");
    let run = sqlx::query_as::<_, AgentRun>(&select_run_sql)
        .bind(&run_id)
        .fetch_one(&mut *tx)
        .await
        .context("failed to load Agent Run")?;

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "agent_run.claimed",
        "agent_run",
        &run.id,
        serde_json::json!({
            "task_id": task.id,
            "task_identifier": task.identifier,
            "queue_key": queue.key,
            "lease_expires_at": run.lease_expires_at,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;
    let task = get_task_detail(pool, &task.identifier)
        .await?
        .with_context(|| format!("claimed Task {} was not found", task.identifier))?;

    Ok(Some(ClaimedRun { run, task }))
}

pub async fn heartbeat_run(
    pool: &SqlitePool,
    run_id: &str,
    lease_seconds: i64,
    actor: &Actor,
) -> Result<AgentRun> {
    validate_worker_actor(actor)?;
    validate_positive_seconds("lease_seconds", lease_seconds)?;
    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let run = sqlx::query_as::<_, AgentRun>(
        r#"
        UPDATE agent_runs
        SET last_heartbeat_at = CURRENT_TIMESTAMP,
            lease_expires_at = datetime('now', '+' || ? || ' seconds')
        WHERE id = ?
          AND outcome IS NULL
          AND lease_expires_at > CURRENT_TIMESTAMP
          AND worker_actor_kind = ?
          AND worker_actor_id = ?
        RETURNING id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                  worker_actor_display_name, worker_id, launcher_kind, lease_expires_at,
                  last_heartbeat_at, outcome, failure_reason, failure_reason_code, created_at, finished_at
        "#,
    )
    .bind(lease_seconds)
    .bind(run_id)
    .bind(&actor.kind)
    .bind(&actor.id)
    .fetch_optional(&mut *tx)
    .await
    .context("failed to heartbeat Agent Run")?
    .with_context(|| format!("active Agent Run {run_id} not found for actor"))?;

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "agent_run.heartbeat",
        "agent_run",
        &run.id,
        serde_json::json!({ "lease_expires_at": run.lease_expires_at }),
    )
    .await?;
    tx.commit().await.context("failed to commit transaction")?;
    Ok(run)
}

pub async fn finish_run(
    pool: &SqlitePool,
    run_id: &str,
    input: &FinishRunInput,
    actor: &Actor,
) -> Result<AgentRun> {
    validate_worker_actor(actor)?;
    validate_run_outcome(&input.outcome)?;
    if let Some(seconds) = input.retry_hold_seconds {
        validate_positive_seconds("retry_hold_seconds", seconds)?;
    }
    let failure_reason_code = failure_reason_code_for_finish(input)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let run = sqlx::query_as::<_, AgentRun>(
        r#"
        UPDATE agent_runs
        SET outcome = ?, failure_reason = ?, failure_reason_code = ?, finished_at = CURRENT_TIMESTAMP
        WHERE id = ?
          AND outcome IS NULL
          AND lease_expires_at > CURRENT_TIMESTAMP
          AND worker_actor_kind = ?
          AND worker_actor_id = ?
        RETURNING id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                  worker_actor_display_name, worker_id, launcher_kind, lease_expires_at,
                  last_heartbeat_at, outcome, failure_reason, failure_reason_code, created_at, finished_at
        "#,
    )
    .bind(&input.outcome)
    .bind(&input.failure_reason)
    .bind(failure_reason_code)
    .bind(run_id)
    .bind(&actor.kind)
    .bind(&actor.id)
    .fetch_optional(&mut *tx)
    .await
    .context("failed to finish Agent Run")?
    .with_context(|| format!("active Agent Run {run_id} not found for actor"))?;

    if input.outcome == "failed" {
        let seconds = input.retry_hold_seconds.unwrap_or(60);
        let reason = input
            .failure_reason
            .clone()
            .unwrap_or_else(|| "Agent Run failed".to_string());
        sqlx::query(
            r#"
            INSERT INTO task_retry_holds (task_id, agent_run_id, hold_until, reason)
            VALUES (?, ?, datetime('now', '+' || ? || ' seconds'), ?)
            ON CONFLICT(task_id) DO UPDATE SET
                agent_run_id = excluded.agent_run_id,
                hold_until = excluded.hold_until,
                reason = excluded.reason,
                created_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&run.task_id)
        .bind(&run.id)
        .bind(seconds)
        .bind(&reason)
        .execute(&mut *tx)
        .await
        .context("failed to create Retry Hold")?;
        append_audit_event_in_tx(
            &mut tx,
            actor,
            "task.retry_hold_created",
            "task",
            &run.task_id,
            serde_json::json!({
                "agent_run_id": run.id,
                "hold_seconds": seconds,
                "reason": reason,
                "failure_reason_code": run.failure_reason_code,
            }),
        )
        .await?;
    } else if input.outcome == "completed" {
        let deleted = sqlx::query("DELETE FROM task_retry_holds WHERE task_id = ?")
            .bind(&run.task_id)
            .execute(&mut *tx)
            .await
            .context("failed to clear Retry Hold")?;
        if deleted.rows_affected() > 0 {
            append_audit_event_in_tx(
                &mut tx,
                actor,
                "task.retry_hold_cleared",
                "task",
                &run.task_id,
                serde_json::json!({ "agent_run_id": run.id }),
            )
            .await?;
        }
    }

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "agent_run.finished",
        "agent_run",
        &run.id,
        serde_json::json!({
            "outcome": input.outcome,
            "failure_reason": input.failure_reason,
            "failure_reason_code": run.failure_reason_code,
            "retry_hold_seconds": input.retry_hold_seconds,
        }),
    )
    .await?;
    tx.commit().await.context("failed to commit transaction")?;
    refresh_agent_run_metrics(pool, &run.id).await?;
    Ok(run)
}

pub async fn operator_fail_run(
    pool: &SqlitePool,
    run_id: &str,
    input: &OperatorFailRunInput,
    actor: &Actor,
) -> Result<AgentRun> {
    validate_operator_actor(actor)?;
    ensure_not_blank("failure reason", &input.failure_reason)?;
    if let Some(seconds) = input.retry_hold_seconds {
        validate_positive_seconds("retry_hold_seconds", seconds)?;
    }
    let failure_reason_code = input
        .failure_reason_code
        .as_deref()
        .unwrap_or("operator_failed");
    validate_failure_reason_code(failure_reason_code)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let run = sqlx::query_as::<_, AgentRun>(
        r#"
        UPDATE agent_runs
        SET outcome = 'failed', failure_reason = ?, failure_reason_code = ?, finished_at = CURRENT_TIMESTAMP
        WHERE id = ? AND outcome IS NULL
        RETURNING id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                  worker_actor_display_name, worker_id, launcher_kind, lease_expires_at,
                  last_heartbeat_at, outcome, failure_reason, failure_reason_code, created_at, finished_at
        "#,
    )
    .bind(input.failure_reason.trim())
    .bind(failure_reason_code)
    .bind(run_id)
    .fetch_optional(&mut *tx)
    .await
    .context("failed to fail Agent Run")?
    .with_context(|| format!("active Agent Run {run_id} not found"))?;

    let seconds = input.retry_hold_seconds.unwrap_or(60);
    sqlx::query(
        r#"
        INSERT INTO task_retry_holds (task_id, agent_run_id, hold_until, reason)
        VALUES (?, ?, datetime('now', '+' || ? || ' seconds'), ?)
        ON CONFLICT(task_id) DO UPDATE SET
            agent_run_id = excluded.agent_run_id,
            hold_until = excluded.hold_until,
            reason = excluded.reason,
            created_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(&run.task_id)
    .bind(&run.id)
    .bind(seconds)
    .bind(input.failure_reason.trim())
    .execute(&mut *tx)
    .await
    .context("failed to create Retry Hold")?;

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task.retry_hold_created",
        "task",
        &run.task_id,
        serde_json::json!({
            "agent_run_id": run.id,
            "hold_seconds": seconds,
            "reason": input.failure_reason.trim(),
            "failure_reason_code": run.failure_reason_code,
        }),
    )
    .await?;
    append_audit_event_in_tx(
        &mut tx,
        actor,
        "agent_run.operator_failed",
        "agent_run",
        &run.id,
        serde_json::json!({
            "reason": input.failure_reason.trim(),
            "failure_reason_code": run.failure_reason_code,
            "retry_hold_seconds": seconds,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;
    refresh_agent_run_metrics(pool, &run.id).await?;
    Ok(run)
}

pub async fn retry_task(
    pool: &SqlitePool,
    identifier: &str,
    input: &RetryTaskInput,
    actor: &Actor,
) -> Result<TaskDetail> {
    validate_operator_actor(actor)?;
    ensure_not_blank("retry reason", &input.reason)?;

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    expire_stale_agent_runs(&mut tx).await?;

    let task = sqlx::query_as::<_, Task>(
        r#"
        SELECT tasks.id, tasks.task_queue_id, task_queues.key AS task_queue_key,
               tasks.identifier, tasks.sequence, tasks.title, tasks.brief, tasks.priority,
               tasks.state, tasks.review_required, tasks.validated_base_commit, tasks.created_at, tasks.updated_at
        FROM tasks
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE tasks.identifier = ?
        "#,
    )
    .bind(identifier)
    .fetch_optional(&mut *tx)
    .await
    .with_context(|| format!("failed to load Task {identifier}"))?
    .with_context(|| format!("Task {identifier} not found"))?;

    if !matches!(
        task.state.as_str(),
        "in_progress" | "rework" | "integrating" | "canceled"
    ) {
        anyhow::bail!(
            "Retry recovery requires Task State in_progress, rework, integrating, or canceled; current state is {}",
            task.state
        );
    }
    ensure_ready_requirements_exist(&mut tx, &task.id).await?;

    let active_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM agent_runs
        WHERE task_id = ? AND outcome IS NULL AND lease_expires_at > CURRENT_TIMESTAMP
        "#,
    )
    .bind(&task.id)
    .fetch_one(&mut *tx)
    .await
    .context("failed to count active Agent Runs")?;
    if active_count > 0 {
        anyhow::bail!("cannot retry Task while it has active Agent Runs");
    }

    let previous_run_outcome: Option<String> = sqlx::query_scalar(
        r#"
        SELECT outcome FROM agent_runs
        WHERE task_id = ?
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(&task.id)
    .fetch_optional(&mut *tx)
    .await
    .context("failed to load latest Agent Run outcome")?;

    let deleted_holds = sqlx::query("DELETE FROM task_retry_holds WHERE task_id = ?")
        .bind(&task.id)
        .execute(&mut *tx)
        .await
        .context("failed to clear Retry Hold")?;
    if deleted_holds.rows_affected() > 0 {
        append_audit_event_in_tx(
            &mut tx,
            actor,
            "task.retry_hold_cleared",
            "task",
            &task.id,
            serde_json::json!({ "identifier": identifier, "reason": "operator retry" }),
        )
        .await?;
    }

    let update = sqlx::query(
        "UPDATE tasks SET state = 'ready', updated_at = CURRENT_TIMESTAMP WHERE id = ? AND state = ?",
    )
    .bind(&task.id)
    .bind(&task.state)
    .execute(&mut *tx)
    .await
    .context("failed to move Task to Ready for retry")?;
    if update.rows_affected() != 1 {
        anyhow::bail!("Task State changed while attempting retry recovery");
    }

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "task.retry_requested",
        "task",
        &task.id,
        serde_json::json!({
            "identifier": identifier,
            "from": task.state,
            "to": "ready",
            "reason": input.reason.trim(),
            "latest_agent_run_outcome": previous_run_outcome,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;
    get_task_detail(pool, identifier)
        .await?
        .with_context(|| format!("retried Task {identifier} was not found"))
}

pub fn is_valid_integration_outcome_reason_code(reason_code: &str) -> bool {
    matches!(
        reason_code,
        "success"
            | "no_changes"
            | "uncommitted_local_worktree"
            | "stale_validated_base_commit"
            | "task_branch_missing_main"
            | "dirty_managed_source_repository"
            | "repo_operation_lock_held"
            | "merge_conflict"
            | "cleanup_failure"
            | "unknown_operational_failure"
            | "unknown_work_change_failure"
            | "unknown_legacy"
    )
}

pub async fn record_integration_outcome(
    pool: &SqlitePool,
    input: &RecordIntegrationOutcomeInput,
    actor: &Actor,
) -> Result<IntegrationOutcome> {
    validate_actor(actor)?;
    if !matches!(
        input.outcome_kind.as_str(),
        "success" | "no_changes" | "work_change_failure" | "operational_failure"
    ) {
        anyhow::bail!("invalid Integration Outcome kind");
    }

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let task_id: String = sqlx::query_scalar("SELECT id FROM tasks WHERE identifier = ?")
        .bind(&input.task_identifier)
        .fetch_optional(&mut *tx)
        .await
        .with_context(|| format!("failed to load Task {}", input.task_identifier))?
        .with_context(|| format!("Task {} not found", input.task_identifier))?;

    if let Some(agent_run_id) = &input.agent_run_id {
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM agent_runs WHERE id = ?")
            .bind(agent_run_id)
            .fetch_optional(&mut *tx)
            .await
            .with_context(|| format!("failed to load Agent Run {agent_run_id}"))?;
        if exists.is_none() {
            anyhow::bail!("Agent Run {agent_run_id} not found");
        }
    }

    if !is_valid_integration_outcome_reason_code(&input.reason_code) {
        anyhow::bail!("invalid Integration Outcome reason code");
    }
    if input.retryable && input.outcome_kind != "operational_failure" {
        anyhow::bail!("only operational Integration Outcomes may be marked retryable");
    }
    if input.retry_attempt.is_some_and(|attempt| attempt <= 0) {
        anyhow::bail!("retry_attempt must be positive");
    }
    if input
        .retry_delay_seconds
        .is_some_and(|seconds| seconds <= 0)
    {
        anyhow::bail!("retry_delay_seconds must be positive");
    }

    let outcome_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO integration_outcomes (
            id, task_id, agent_run_id, outcome_kind, reason_code, final_commit, pre_merge_head, message,
            retryable, retry_attempt, next_retry_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CASE WHEN ? IS NULL THEN NULL ELSE datetime('now', '+' || ? || ' seconds') END)
        "#,
    )
    .bind(&outcome_id)
    .bind(&task_id)
    .bind(&input.agent_run_id)
    .bind(&input.outcome_kind)
    .bind(&input.reason_code)
    .bind(&input.final_commit)
    .bind(&input.pre_merge_head)
    .bind(&input.message)
    .bind(input.retryable)
    .bind(input.retry_attempt)
    .bind(input.retry_delay_seconds)
    .bind(input.retry_delay_seconds)
    .execute(&mut *tx)
    .await
    .context("failed to record Integration Outcome")?;

    append_audit_event_in_tx(
        &mut tx,
        actor,
        "integration_outcome.recorded",
        "task",
        &task_id,
        serde_json::json!({
            "identifier": input.task_identifier,
            "agent_run_id": input.agent_run_id,
            "outcome_kind": input.outcome_kind,
            "reason_code": input.reason_code,
            "final_commit": input.final_commit,
            "pre_merge_head": input.pre_merge_head,
            "message": input.message,
            "retryable": input.retryable,
            "retry_attempt": input.retry_attempt,
            "retry_delay_seconds": input.retry_delay_seconds,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;

    sqlx::query_as::<_, IntegrationOutcome>(
        r#"
        SELECT id, task_id, agent_run_id, outcome_kind, reason_code, final_commit, pre_merge_head, message,
               retryable, retry_attempt, next_retry_at, created_at
        FROM integration_outcomes
        WHERE id = ?
        "#,
    )
    .bind(outcome_id)
    .fetch_one(pool)
    .await
    .context("failed to load recorded Integration Outcome")
}

pub async fn get_agent_run(pool: &SqlitePool, run_id: &str) -> Result<Option<AgentRun>> {
    let select_run_sql = agent_run_select_sql("WHERE id = ?");
    sqlx::query_as::<_, AgentRun>(&select_run_sql)
        .bind(run_id)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("failed to load Agent Run {run_id}"))
}

pub async fn upsert_launcher_session_data(
    pool: &SqlitePool,
    agent_run_id: &str,
    input: &UpsertLauncherSessionData,
    actor: &Actor,
) -> Result<LauncherSessionData> {
    validate_actor(actor)?;
    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let run_exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM agent_runs WHERE id = ?")
        .bind(agent_run_id)
        .fetch_optional(&mut *tx)
        .await
        .with_context(|| format!("failed to load Agent Run {agent_run_id}"))?;
    if run_exists.is_none() {
        anyhow::bail!("Agent Run {agent_run_id} not found");
    }
    sqlx::query(
        r#"
        INSERT INTO launcher_session_data (
            agent_run_id, launcher_kind, session_id, model, provider, started_at, finished_at,
            final_status, transcript_path, raw_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(agent_run_id) DO UPDATE SET
            launcher_kind = excluded.launcher_kind,
            session_id = excluded.session_id,
            model = excluded.model,
            provider = excluded.provider,
            started_at = excluded.started_at,
            finished_at = excluded.finished_at,
            final_status = excluded.final_status,
            transcript_path = excluded.transcript_path,
            raw_json = excluded.raw_json,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(agent_run_id)
    .bind(&input.launcher_kind)
    .bind(&input.session_id)
    .bind(&input.model)
    .bind(&input.provider)
    .bind(&input.started_at)
    .bind(&input.finished_at)
    .bind(&input.final_status)
    .bind(&input.transcript_path)
    .bind(&input.raw_json)
    .execute(&mut *tx)
    .await
    .context("failed to upsert Launcher Session Data")?;
    append_audit_event_in_tx(
        &mut tx,
        actor,
        "agent_run.launcher_session_data_recorded",
        "agent_run",
        agent_run_id,
        serde_json::json!({
            "launcher_kind": input.launcher_kind,
            "session_id": input.session_id,
            "final_status": input.final_status,
            "transcript_path": input.transcript_path,
        }),
    )
    .await?;
    tx.commit().await.context("failed to commit transaction")?;
    refresh_agent_run_metrics(pool, agent_run_id).await?;
    get_launcher_session_data(pool, agent_run_id)
        .await?
        .with_context(|| format!("Launcher Session Data for Agent Run {agent_run_id} not found"))
}

pub async fn get_launcher_session_data(
    pool: &SqlitePool,
    agent_run_id: &str,
) -> Result<Option<LauncherSessionData>> {
    sqlx::query_as::<_, LauncherSessionData>(
        r#"
        SELECT agent_run_id, launcher_kind, session_id, model, provider, started_at, finished_at,
               final_status, transcript_path, raw_json, created_at, updated_at
        FROM launcher_session_data
        WHERE agent_run_id = ?
        "#,
    )
    .bind(agent_run_id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to load Launcher Session Data for Agent Run {agent_run_id}"))
}

pub async fn get_agent_run_metrics(
    pool: &SqlitePool,
    agent_run_id: &str,
) -> Result<Option<AgentRunMetrics>> {
    sqlx::query_as::<_, AgentRunMetrics>(
        r#"
        SELECT agent_run_id, duration_ms, launcher_kind, final_status, exit_code, timed_out,
               unattended_question_detected, blocking_ui_detected, transcript_path,
               transcript_byte_size, transcript_jsonl_event_count, input_tokens, output_tokens,
               total_tokens, cache_read_tokens, cache_write_tokens, tool_call_count, tool_error_count,
               repeated_failed_tool_attempt_count, tool_call_counts_json, repeated_read_count,
               repeated_tasker_context_fetch_count, shell_command_counts_json,
               assistant_turn_count, user_turn_count, max_context_tokens, efficiency_hints_json,
               warnings_json, created_at, updated_at
        FROM agent_run_metrics
        WHERE agent_run_id = ?
        "#,
    )
    .bind(agent_run_id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to load Agent Run metrics for Agent Run {agent_run_id}"))
}

pub async fn compute_agent_run_metrics(
    pool: &SqlitePool,
    agent_run_id: &str,
) -> Result<Option<ComputedAgentRunMetrics>> {
    let Some(run) = get_agent_run(pool, agent_run_id).await? else {
        anyhow::bail!("Agent Run {agent_run_id} not found");
    };
    if run.outcome.is_none() {
        return Ok(None);
    }
    let session = get_launcher_session_data(pool, agent_run_id).await?;
    let mut summary = AgentRunMetricsSummary::default();
    if let Some(session) = &session {
        summary.launcher_kind = session.launcher_kind.clone();
        summary.final_status = session.final_status.clone().or_else(|| run.outcome.clone());
        summary.transcript_path = session.transcript_path.clone();
        summary.observe_launcher_raw_json(session.raw_json.as_deref());
        if let Some(path) = &session.transcript_path {
            summary.observe_transcript(Path::new(path));
        }
    } else {
        summary.launcher_kind = run.launcher_kind.clone();
        summary.final_status = run.outcome.clone();
        summary
            .warnings
            .push("Launcher Session Data not recorded".to_string());
    }
    let warnings_json = serde_json::to_string(&summary.warnings)
        .context("failed to serialize Agent Run metrics warnings")?;
    let duration_ms: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT CAST((julianday(finished_at) - julianday(created_at)) * 86400000 AS INTEGER)
        FROM agent_runs
        WHERE id = ? AND finished_at IS NOT NULL
        "#,
    )
    .bind(agent_run_id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to compute Agent Run duration for Agent Run {agent_run_id}"))?
    .flatten();
    let tool_call_counts_json = summary.tool_call_counts_json()?;
    let shell_command_counts_json = summary.shell_command_counts_json()?;
    let efficiency_hints_json = summary.efficiency_hints_json()?;
    Ok(Some(ComputedAgentRunMetrics {
        agent_run_id: agent_run_id.to_string(),
        duration_ms,
        launcher_kind: summary.launcher_kind,
        final_status: summary.final_status,
        exit_code: summary.exit_code,
        timed_out: summary.timed_out.map(bool_to_i64),
        unattended_question_detected: summary.unattended_question_detected.map(bool_to_i64),
        blocking_ui_detected: summary.blocking_ui_detected.map(bool_to_i64),
        transcript_path: summary.transcript_path,
        transcript_byte_size: summary.transcript_byte_size,
        transcript_jsonl_event_count: summary.transcript_jsonl_event_count,
        input_tokens: summary.input_tokens,
        output_tokens: summary.output_tokens,
        total_tokens: summary.total_tokens,
        cache_read_tokens: summary.cache_read_tokens,
        cache_write_tokens: summary.cache_write_tokens,
        tool_call_count: summary.tool_call_count,
        tool_error_count: summary.tool_error_count,
        repeated_failed_tool_attempt_count: summary.repeated_failed_tool_attempt_count,
        tool_call_counts_json,
        repeated_read_count: summary.repeated_read_count,
        repeated_tasker_context_fetch_count: summary.repeated_tasker_context_fetch_count,
        shell_command_counts_json,
        assistant_turn_count: summary.assistant_turn_count,
        user_turn_count: summary.user_turn_count,
        max_context_tokens: summary.max_context_tokens,
        efficiency_hints_json,
        warnings_json,
    }))
}

pub async fn refresh_agent_run_metrics(
    pool: &SqlitePool,
    agent_run_id: &str,
) -> Result<Option<AgentRunMetrics>> {
    let Some(metrics) = compute_agent_run_metrics(pool, agent_run_id).await? else {
        return Ok(None);
    };
    sqlx::query(
        r#"
        INSERT INTO agent_run_metrics (
            agent_run_id, duration_ms, launcher_kind, final_status, exit_code, timed_out,
            unattended_question_detected, blocking_ui_detected, transcript_path,
            transcript_byte_size, transcript_jsonl_event_count, input_tokens, output_tokens,
            total_tokens, cache_read_tokens, cache_write_tokens, tool_call_count, tool_error_count,
            repeated_failed_tool_attempt_count, tool_call_counts_json, repeated_read_count,
            repeated_tasker_context_fetch_count, shell_command_counts_json,
            assistant_turn_count, user_turn_count, max_context_tokens, efficiency_hints_json, warnings_json
        )
        SELECT
            agent_runs.id,
            ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
        FROM agent_runs
        WHERE agent_runs.id = ? AND agent_runs.outcome IS NOT NULL
        ON CONFLICT(agent_run_id) DO UPDATE SET
            duration_ms = excluded.duration_ms,
            launcher_kind = excluded.launcher_kind,
            final_status = excluded.final_status,
            exit_code = excluded.exit_code,
            timed_out = excluded.timed_out,
            unattended_question_detected = excluded.unattended_question_detected,
            blocking_ui_detected = excluded.blocking_ui_detected,
            transcript_path = excluded.transcript_path,
            transcript_byte_size = excluded.transcript_byte_size,
            transcript_jsonl_event_count = excluded.transcript_jsonl_event_count,
            input_tokens = excluded.input_tokens,
            output_tokens = excluded.output_tokens,
            total_tokens = excluded.total_tokens,
            cache_read_tokens = excluded.cache_read_tokens,
            cache_write_tokens = excluded.cache_write_tokens,
            tool_call_count = excluded.tool_call_count,
            tool_error_count = excluded.tool_error_count,
            repeated_failed_tool_attempt_count = excluded.repeated_failed_tool_attempt_count,
            tool_call_counts_json = excluded.tool_call_counts_json,
            repeated_read_count = excluded.repeated_read_count,
            repeated_tasker_context_fetch_count = excluded.repeated_tasker_context_fetch_count,
            shell_command_counts_json = excluded.shell_command_counts_json,
            assistant_turn_count = excluded.assistant_turn_count,
            user_turn_count = excluded.user_turn_count,
            max_context_tokens = excluded.max_context_tokens,
            efficiency_hints_json = excluded.efficiency_hints_json,
            warnings_json = excluded.warnings_json,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(metrics.duration_ms)
    .bind(&metrics.launcher_kind)
    .bind(&metrics.final_status)
    .bind(metrics.exit_code)
    .bind(metrics.timed_out)
    .bind(metrics.unattended_question_detected)
    .bind(metrics.blocking_ui_detected)
    .bind(&metrics.transcript_path)
    .bind(metrics.transcript_byte_size)
    .bind(metrics.transcript_jsonl_event_count)
    .bind(metrics.input_tokens)
    .bind(metrics.output_tokens)
    .bind(metrics.total_tokens)
    .bind(metrics.cache_read_tokens)
    .bind(metrics.cache_write_tokens)
    .bind(metrics.tool_call_count)
    .bind(metrics.tool_error_count)
    .bind(metrics.repeated_failed_tool_attempt_count)
    .bind(&metrics.tool_call_counts_json)
    .bind(metrics.repeated_read_count)
    .bind(metrics.repeated_tasker_context_fetch_count)
    .bind(&metrics.shell_command_counts_json)
    .bind(metrics.assistant_turn_count)
    .bind(metrics.user_turn_count)
    .bind(metrics.max_context_tokens)
    .bind(&metrics.efficiency_hints_json)
    .bind(&metrics.warnings_json)
    .bind(agent_run_id)
    .execute(pool)
    .await
    .with_context(|| format!("failed to persist Agent Run metrics for Agent Run {agent_run_id}"))?;
    get_agent_run_metrics(pool, agent_run_id).await
}

#[derive(Debug, Default)]
struct AgentRunMetricsSummary {
    launcher_kind: String,
    final_status: Option<String>,
    exit_code: Option<i64>,
    timed_out: Option<bool>,
    unattended_question_detected: Option<bool>,
    blocking_ui_detected: Option<bool>,
    transcript_path: Option<String>,
    transcript_byte_size: Option<i64>,
    transcript_jsonl_event_count: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    total_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    tool_call_count: Option<i64>,
    tool_error_count: Option<i64>,
    repeated_failed_tool_attempt_count: Option<i64>,
    tool_call_counts: std::collections::BTreeMap<String, i64>,
    repeated_read_count: Option<i64>,
    repeated_tasker_context_fetch_count: Option<i64>,
    shell_command_counts: std::collections::BTreeMap<String, i64>,
    read_paths: std::collections::HashMap<String, i64>,
    tasker_context_fetch_signatures: std::collections::HashMap<String, i64>,
    seen_tool_call_ids: std::collections::HashSet<String>,
    seen_tool_detail_ids: std::collections::HashSet<String>,
    assistant_turn_count: Option<i64>,
    user_turn_count: Option<i64>,
    max_context_tokens: Option<i64>,
    failed_tool_signatures: std::collections::HashMap<String, i64>,
    warnings: Vec<String>,
}

impl AgentRunMetricsSummary {
    fn observe_launcher_raw_json(&mut self, raw_json: Option<&str>) {
        let Some(raw_json) = raw_json else { return };
        match serde_json::from_str::<serde_json::Value>(raw_json) {
            Ok(value) => {
                self.exit_code = self.exit_code.or_else(|| json_i64(&value, &["exit_code"]));
                self.timed_out = self.timed_out.or_else(|| json_bool(&value, &["timed_out"]));
                self.unattended_question_detected = self
                    .unattended_question_detected
                    .or_else(|| json_bool(&value, &["unattended_question_detected"]));
                self.observe_token_usage(&value);
            }
            Err(error) => self.warnings.push(format!(
                "ignored malformed Launcher Session Data raw JSON: {error}"
            )),
        }
    }

    fn observe_transcript(&mut self, path: &Path) {
        match fs::metadata(path) {
            Ok(metadata) => self.transcript_byte_size = Some(metadata.len() as i64),
            Err(error) => {
                self.warnings.push(format!(
                    "could not stat Run Transcript {}: {error}",
                    path.display()
                ));
            }
        }
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                self.warnings.push(format!(
                    "could not read Run Transcript {}: {error}",
                    path.display()
                ));
                return;
            }
        };
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(value) => {
                    self.transcript_jsonl_event_count =
                        Some(self.transcript_jsonl_event_count.unwrap_or(0) + 1);
                    self.observe_transcript_record(&value);
                }
                Err(error) => self.warnings.push(format!(
                    "ignored malformed Run Transcript line {}: {error}",
                    index + 1
                )),
            }
        }
    }

    fn observe_transcript_record(&mut self, value: &serde_json::Value) {
        self.observe_event(value);
        self.exit_code = self.exit_code.or_else(|| json_i64(value, &["status"]));
        self.timed_out = self.timed_out.or_else(|| json_bool(value, &["timed_out"]));
        self.unattended_question_detected = self
            .unattended_question_detected
            .or_else(|| json_bool(value, &["unattended_question_detected"]));
        for field in ["stdout", "stderr"] {
            if let Some(text) = value.get(field).and_then(|value| value.as_str()) {
                for (index, line) in text.lines().enumerate() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() || !trimmed.starts_with('{') {
                        continue;
                    }
                    match serde_json::from_str::<serde_json::Value>(trimmed) {
                        Ok(value) => self.observe_event(&value),
                        Err(error) => self.warnings.push(format!(
                            "ignored malformed JSON event in {field} line {}: {error}",
                            index + 1
                        )),
                    }
                }
            }
        }
    }

    fn observe_event(&mut self, value: &serde_json::Value) {
        if value.get("type").and_then(|value| value.as_str()) == Some("extension_ui_request") {
            let method = value
                .get("method")
                .or_else(|| value.get("method_name"))
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            if method != "notify" {
                self.blocking_ui_detected = Some(true);
            }
        }
        if value.get("event").and_then(|value| value.as_str()) == Some("question") {
            self.unattended_question_detected = Some(true);
        }
        self.observe_roles_and_usage(value);
        self.observe_tool_event(value);
        self.observe_nested_tool_events(value);
    }

    fn observe_nested_tool_events(&mut self, value: &serde_json::Value) {
        for path in ["/message/content", "/assistantMessageEvent/partial/content"] {
            if let Some(content) = value.pointer(path).and_then(|value| value.as_array()) {
                for item in content {
                    self.observe_tool_event(item);
                }
            }
        }
    }

    fn observe_roles_and_usage(&mut self, value: &serde_json::Value) {
        if let Some(role) = value.get("role").and_then(|value| value.as_str()) {
            match role {
                "assistant" => {
                    self.assistant_turn_count = Some(self.assistant_turn_count.unwrap_or(0) + 1)
                }
                "user" => self.user_turn_count = Some(self.user_turn_count.unwrap_or(0) + 1),
                _ => {}
            }
        }
        self.observe_token_usage(value);
    }

    fn observe_token_usage(&mut self, value: &serde_json::Value) {
        if let Some(input) = first_json_i64(
            value,
            &[
                &["input_tokens"],
                &["inputTokens"],
                &["usage", "input_tokens"],
                &["usage", "inputTokens"],
                &["usage", "input"],
                &["usage", "prompt_tokens"],
                &["message", "usage", "input"],
                &["message", "usage", "input_tokens"],
                &["message", "usage", "inputTokens"],
                &["assistantMessageEvent", "partial", "usage", "input"],
                &["assistantMessageEvent", "partial", "usage", "input_tokens"],
                &["assistantMessageEvent", "partial", "usage", "inputTokens"],
            ],
        ) {
            self.input_tokens = Some(self.input_tokens.unwrap_or(0).max(input));
        }
        if let Some(output) = first_json_i64(
            value,
            &[
                &["output_tokens"],
                &["outputTokens"],
                &["usage", "output_tokens"],
                &["usage", "outputTokens"],
                &["usage", "output"],
                &["usage", "completion_tokens"],
                &["message", "usage", "output"],
                &["message", "usage", "output_tokens"],
                &["message", "usage", "outputTokens"],
                &["assistantMessageEvent", "partial", "usage", "output"],
                &["assistantMessageEvent", "partial", "usage", "output_tokens"],
                &["assistantMessageEvent", "partial", "usage", "outputTokens"],
            ],
        ) {
            self.output_tokens = Some(self.output_tokens.unwrap_or(0).max(output));
        }
        if let Some(total) = first_json_i64(
            value,
            &[
                &["total_tokens"],
                &["totalTokens"],
                &["usage", "total_tokens"],
                &["usage", "totalTokens"],
                &["message", "usage", "total_tokens"],
                &["message", "usage", "totalTokens"],
                &["assistantMessageEvent", "partial", "usage", "total_tokens"],
                &["assistantMessageEvent", "partial", "usage", "totalTokens"],
            ],
        ) {
            self.total_tokens = Some(self.total_tokens.unwrap_or(0).max(total));
            self.max_context_tokens = Some(self.max_context_tokens.unwrap_or(0).max(total));
        }
        if let Some(cache_read) = first_json_i64(
            value,
            &[
                &["cache_read_tokens"],
                &["cacheReadTokens"],
                &["usage", "cache_read_tokens"],
                &["usage", "cacheReadTokens"],
                &["usage", "cacheRead"],
                &["message", "usage", "cacheRead"],
                &["assistantMessageEvent", "partial", "usage", "cacheRead"],
            ],
        ) {
            self.cache_read_tokens = Some(self.cache_read_tokens.unwrap_or(0).max(cache_read));
        }
        if let Some(cache_write) = first_json_i64(
            value,
            &[
                &["cache_write_tokens"],
                &["cacheWriteTokens"],
                &["usage", "cache_write_tokens"],
                &["usage", "cacheWriteTokens"],
                &["usage", "cacheWrite"],
                &["message", "usage", "cacheWrite"],
                &["assistantMessageEvent", "partial", "usage", "cacheWrite"],
            ],
        ) {
            self.cache_write_tokens = Some(self.cache_write_tokens.unwrap_or(0).max(cache_write));
        }
        if let Some(context) = first_json_i64(
            value,
            &[
                &["context_tokens"],
                &["contextTokens"],
                &["max_context_tokens"],
                &["maxContextTokens"],
                &["usage", "context_tokens"],
                &["usage", "contextTokens"],
                &["usage", "max_context_tokens"],
                &["usage", "maxContextTokens"],
                &["message", "usage", "context_tokens"],
                &["message", "usage", "contextTokens"],
                &[
                    "assistantMessageEvent",
                    "partial",
                    "usage",
                    "context_tokens",
                ],
                &["assistantMessageEvent", "partial", "usage", "contextTokens"],
            ],
        ) {
            self.max_context_tokens = Some(self.max_context_tokens.unwrap_or(0).max(context));
        }
    }

    fn observe_tool_event(&mut self, value: &serde_json::Value) {
        let type_text = value
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let tool_name = tool_name(value);
        let has_tool_name = tool_name.is_some();
        let is_tool_delta = type_text.contains("toolcall_delta")
            || type_text.contains("tool_call_delta")
            || type_text.contains("tool_delta");
        let is_tool_call = !is_tool_delta
            && ((type_text.contains("tool")
                && (type_text.contains("call")
                    || type_text.contains("use")
                    || type_text.contains("start")
                    || type_text.contains("execution")))
                || type_text == "function_call"
                || value.get("function_call").is_some());
        if is_tool_call {
            let call_id = tool_call_id(value);
            let already_counted = call_id
                .as_ref()
                .is_some_and(|call_id| self.seen_tool_call_ids.contains(call_id));
            let name = tool_name.unwrap_or_else(|| "unknown".to_string());
            if !already_counted {
                if let Some(call_id) = &call_id {
                    self.seen_tool_call_ids.insert(call_id.clone());
                }
                self.tool_call_count = Some(self.tool_call_count.unwrap_or(0) + 1);
                *self.tool_call_counts.entry(name.clone()).or_insert(0) += 1;
            }
            let details_already_observed = call_id
                .as_ref()
                .is_some_and(|call_id| self.seen_tool_detail_ids.contains(call_id));
            if !details_already_observed && self.observe_tool_call_details(&name, value) {
                if let Some(call_id) = call_id {
                    self.seen_tool_detail_ids.insert(call_id);
                }
            }
        }
        let status = value
            .get("status")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let is_error = value
            .get("is_error")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
            || status == "error"
            || status == "failed"
            || type_text.contains("error");
        if is_error && (has_tool_name || type_text.contains("tool")) {
            self.tool_error_count = Some(self.tool_error_count.unwrap_or(0) + 1);
            let signature = tool_signature(value);
            let count = self.failed_tool_signatures.entry(signature).or_insert(0);
            *count += 1;
            if *count > 1 {
                self.repeated_failed_tool_attempt_count =
                    Some(self.repeated_failed_tool_attempt_count.unwrap_or(0) + 1);
            }
        }
    }

    fn observe_tool_call_details(&mut self, name: &str, value: &serde_json::Value) -> bool {
        let canonical = canonical_tool_name(name);
        if canonical == "read" {
            if let Some(path) = tool_string_arg(value, &["path", "file", "filename"]) {
                let count = self.read_paths.entry(path).or_insert(0);
                *count += 1;
                if *count > 1 {
                    self.repeated_read_count = Some(self.repeated_read_count.unwrap_or(0) + 1);
                }
                return true;
            }
            return false;
        }
        if canonical == "bash" {
            if let Some(command) = tool_string_arg(value, &["command", "cmd"]) {
                let category = shell_command_category(&command);
                *self
                    .shell_command_counts
                    .entry(category.to_string())
                    .or_insert(0) += 1;
                if let Some(signature) = tasker_context_fetch_signature(&command) {
                    let count = self
                        .tasker_context_fetch_signatures
                        .entry(signature)
                        .or_insert(0);
                    *count += 1;
                    if *count > 1 {
                        self.repeated_tasker_context_fetch_count =
                            Some(self.repeated_tasker_context_fetch_count.unwrap_or(0) + 1);
                    }
                }
                return true;
            }
            return false;
        } else if is_tasker_context_tool(&canonical) {
            let count = self
                .tasker_context_fetch_signatures
                .entry(format!("tool:{canonical}"))
                .or_insert(0);
            *count += 1;
            if *count > 1 {
                self.repeated_tasker_context_fetch_count =
                    Some(self.repeated_tasker_context_fetch_count.unwrap_or(0) + 1);
            }
            return true;
        }
        false
    }

    fn tool_call_counts_json(&self) -> Result<String> {
        serde_json::to_string(&self.tool_call_counts)
            .context("failed to serialize Agent Run per-tool counts")
    }

    fn shell_command_counts_json(&self) -> Result<String> {
        serde_json::to_string(&self.shell_command_counts)
            .context("failed to serialize Agent Run shell command counts")
    }

    fn efficiency_hints_json(&self) -> Result<String> {
        let mut hints = Vec::new();
        if self.tool_call_count.unwrap_or(0) >= 30 {
            hints.push("excessive tool calls".to_string());
        }
        if self.repeated_failed_tool_attempt_count.unwrap_or(0) > 0 {
            hints.push("repeated failed tool attempts".to_string());
        }
        if self.repeated_read_count.unwrap_or(0) > 0 {
            hints.push("repeated file reads".to_string());
        }
        if self.repeated_tasker_context_fetch_count.unwrap_or(0) > 0 {
            hints.push("repeated Tasker context fetches".to_string());
        }
        if self.transcript_byte_size.unwrap_or(0) >= 10_000_000 {
            hints.push("large transcript/proxy output volume".to_string());
        }
        if self.max_context_tokens.unwrap_or(0) >= 100_000 {
            hints.push("large context growth".to_string());
        }
        if self.blocking_ui_detected == Some(true)
            || self.unattended_question_detected == Some(true)
        {
            hints.push("unexpected UI/questions".to_string());
        }
        if self.tool_error_count.unwrap_or(0) >= 5 {
            hints.push("validation/tool loop".to_string());
        }
        serde_json::to_string(&hints).context("failed to serialize Agent Run efficiency hints")
    }
}

fn tool_name(value: &serde_json::Value) -> Option<String> {
    let raw = value
        .get("tool_name")
        .or_else(|| value.get("toolName"))
        .or_else(|| value.get("tool"))
        .or_else(|| value.get("name"))
        .and_then(|value| value.as_str())
        .or_else(|| {
            value
                .pointer("/function/name")
                .and_then(|value| value.as_str())
        })
        .or_else(|| {
            value
                .pointer("/function_call/name")
                .and_then(|value| value.as_str())
        })
        .or_else(|| {
            value
                .pointer("/toolCall/name")
                .and_then(|value| value.as_str())
        })?;
    Some(sanitize_metric_key(raw))
}

fn tool_call_id(value: &serde_json::Value) -> Option<String> {
    value
        .get("id")
        .or_else(|| value.get("tool_call_id"))
        .or_else(|| value.get("toolCallId"))
        .or_else(|| value.get("call_id"))
        .and_then(|value| value.as_str())
        .map(sanitize_metric_key)
}

fn sanitize_metric_key(raw: &str) -> String {
    let lowered = raw.trim().to_ascii_lowercase();
    let sanitized: String = lowered
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn canonical_tool_name(name: &str) -> String {
    name.rsplit(['.', ':']).next().unwrap_or(name).to_string()
}

fn tool_args(value: &serde_json::Value) -> Option<&serde_json::Value> {
    value
        .get("args")
        .or_else(|| value.get("arguments"))
        .or_else(|| value.get("input"))
        .or_else(|| value.get("partialJson"))
        .or_else(|| value.pointer("/function/arguments"))
        .or_else(|| value.pointer("/function_call/arguments"))
}

fn tool_string_arg(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    let args = tool_args(value)?;
    if let Some(text) = args.as_str() {
        if keys.iter().any(|key| matches!(*key, "command" | "cmd")) {
            return Some(text.to_string());
        }
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) {
            return string_arg_from_object(&parsed, keys);
        }
        return None;
    }
    string_arg_from_object(args, keys)
}

fn string_arg_from_object(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(|value| value.as_str()) {
            return Some(text.trim().to_string());
        }
    }
    None
}

fn shell_command_category(command: &str) -> &'static str {
    let lowered = command.trim_start().to_ascii_lowercase();
    if lowered.contains("tasker-local")
        || lowered.starts_with("tasker ")
        || lowered.contains(" tasker ")
        || lowered.contains("cargo run -p tasker-cli")
    {
        "tasker_cli"
    } else if lowered.starts_with("cargo ") || lowered.contains(" cargo ") {
        "cargo"
    } else if lowered.starts_with("git ") || lowered.contains(" git ") {
        "git"
    } else if lowered.starts_with("rg ")
        || lowered.contains(" rg ")
        || lowered.starts_with("find ")
        || lowered.contains(" find ")
        || lowered.starts_with("grep ")
        || lowered.contains(" grep ")
    {
        "search"
    } else if lowered.starts_with("ls")
        || lowered.contains(" ls ")
        || lowered.starts_with("pwd")
        || lowered.contains(" pwd")
        || lowered.starts_with("tree")
        || lowered.contains(" tree ")
    {
        "filesystem"
    } else if lowered.starts_with("npm ")
        || lowered.contains(" npm ")
        || lowered.starts_with("pnpm ")
        || lowered.contains(" pnpm ")
        || lowered.starts_with("yarn ")
        || lowered.contains(" yarn ")
        || lowered.starts_with("bun ")
        || lowered.contains(" bun ")
        || lowered.starts_with("make ")
        || lowered.contains(" make ")
    {
        "package_build"
    } else {
        "other"
    }
}

fn tasker_context_fetch_signature(command: &str) -> Option<String> {
    let lowered = command.to_ascii_lowercase();
    let normalized = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
    let context_kind = if normalized.contains("tasker-local task show")
        || normalized.contains("tasker task show")
        || normalized.contains("task show")
    {
        "task_show"
    } else if normalized.contains("tasker-local queue show")
        || normalized.contains("tasker queue show")
        || normalized.contains("queue show")
    {
        "queue_show"
    } else if normalized.contains("tasker-local run show")
        || normalized.contains("tasker run show")
        || normalized.contains("run show")
    {
        "run_show"
    } else if normalized.contains("tasker-local status")
        || normalized.contains("tasker status")
        || normalized == "status"
    {
        "status"
    } else {
        return None;
    };
    Some(context_kind.to_string())
}

fn is_tasker_context_tool(canonical: &str) -> bool {
    canonical.contains("get_task")
        || canonical.contains("task_context")
        || canonical.contains("task_show")
        || canonical.contains("queue_show")
        || canonical == "status"
}

fn tool_signature(value: &serde_json::Value) -> String {
    let name = tool_name(value).unwrap_or_else(|| "unknown".to_string());
    let args = value
        .get("args")
        .or_else(|| value.get("arguments"))
        .or_else(|| value.get("input"))
        .map(|value| value.to_string())
        .unwrap_or_default();
    format!("{name}:{args}")
}

fn bool_to_i64(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn json_i64(value: &serde_json::Value, path: &[&str]) -> Option<i64> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_i64()
}

fn json_bool(value: &serde_json::Value, path: &[&str]) -> Option<bool> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_bool()
}

fn first_json_i64(value: &serde_json::Value, paths: &[&[&str]]) -> Option<i64> {
    paths.iter().find_map(|path| json_i64(value, path))
}

pub async fn get_agent_run_detail(
    pool: &SqlitePool,
    run_id: &str,
) -> Result<Option<AgentRunDetail>> {
    let Some(run) = get_agent_run(pool, run_id).await? else {
        return Ok(None);
    };
    agent_run_detail_for_run(pool, run).await.map(Some)
}

pub async fn get_latest_agent_run_detail_for_task(
    pool: &SqlitePool,
    identifier: &str,
) -> Result<Option<AgentRunDetail>> {
    let select_run_sql = agent_run_select_sql(
        r#"
        JOIN tasks ON tasks.id = agent_runs.task_id
        WHERE tasks.identifier = ?
        ORDER BY agent_runs.created_at DESC, agent_runs.id DESC
        LIMIT 1
        "#,
    );
    let Some(run) = sqlx::query_as::<_, AgentRun>(&select_run_sql)
        .bind(identifier)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("failed to load latest Agent Run for Task {identifier}"))?
    else {
        return Ok(None);
    };
    agent_run_detail_for_run(pool, run).await.map(Some)
}

async fn agent_run_detail_for_run(pool: &SqlitePool, run: AgentRun) -> Result<AgentRunDetail> {
    let identifier: String = sqlx::query_scalar("SELECT identifier FROM tasks WHERE id = ?")
        .bind(&run.task_id)
        .fetch_one(pool)
        .await
        .with_context(|| format!("failed to load Task for Agent Run {}", run.id))?;
    let task = get_task_detail(pool, &identifier)
        .await?
        .with_context(|| format!("Task {identifier} for Agent Run {} not found", run.id))?;
    let launcher_session_data = get_launcher_session_data(pool, &run.id).await?;
    let metrics = get_agent_run_metrics(pool, &run.id).await?;
    Ok(AgentRunDetail {
        run,
        task,
        launcher_session_data,
        metrics,
    })
}

async fn expire_stale_agent_runs(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<()> {
    let expired = sqlx::query_as::<_, AgentRun>(
        r#"
        UPDATE agent_runs
        SET outcome = 'expired', finished_at = CURRENT_TIMESTAMP, failure_reason = 'Claim Lease expired', failure_reason_code = 'claim_lease_expired'
        WHERE outcome IS NULL AND lease_expires_at <= CURRENT_TIMESTAMP
        RETURNING id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                  worker_actor_display_name, worker_id, launcher_kind, lease_expires_at,
                  last_heartbeat_at, outcome, failure_reason, failure_reason_code, created_at, finished_at
        "#,
    )
    .fetch_all(&mut **tx)
    .await
    .context("failed to expire stale Agent Runs")?;

    for run in expired {
        sqlx::query(
            r#"
            INSERT INTO task_retry_holds (task_id, agent_run_id, hold_until, reason)
            VALUES (?, ?, datetime('now', '+60 seconds'), 'Claim Lease expired')
            ON CONFLICT(task_id) DO UPDATE SET
                agent_run_id = excluded.agent_run_id,
                hold_until = excluded.hold_until,
                reason = excluded.reason,
                created_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&run.task_id)
        .bind(&run.id)
        .execute(&mut **tx)
        .await
        .context("failed to create Retry Hold for expired Agent Run")?;
        let actor = Actor {
            kind: run.worker_actor_kind.clone(),
            id: run.worker_actor_id.clone(),
            display_name: run.worker_actor_display_name.clone(),
        };
        append_audit_event_in_tx(
            tx,
            &actor,
            "task.retry_hold_created",
            "task",
            &run.task_id,
            serde_json::json!({
                "agent_run_id": run.id,
                "hold_seconds": 60,
                "reason": "Claim Lease expired",
                "failure_reason_code": run.failure_reason_code,
            }),
        )
        .await?;
        append_audit_event_in_tx(
            tx,
            &actor,
            "agent_run.expired",
            "agent_run",
            &run.id,
            serde_json::json!({ "reason": "Claim Lease expired", "failure_reason_code": run.failure_reason_code }),
        )
        .await?;
        sqlx::query(
            r#"
            INSERT INTO agent_run_metrics (
                agent_run_id, duration_ms, launcher_kind, final_status, warnings_json
            )
            SELECT
                id,
                CAST((julianday(finished_at) - julianday(created_at)) * 86400000 AS INTEGER),
                launcher_kind,
                outcome,
                '["Launcher Session Data not recorded"]'
            FROM agent_runs
            WHERE id = ?
            ON CONFLICT(agent_run_id) DO UPDATE SET
                duration_ms = excluded.duration_ms,
                launcher_kind = excluded.launcher_kind,
                final_status = excluded.final_status,
                warnings_json = excluded.warnings_json,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&run.id)
        .execute(&mut **tx)
        .await
        .context("failed to persist expired Agent Run metrics")?;
    }

    Ok(())
}

async fn append_audit_event_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    actor: &Actor,
    event_type: &str,
    subject_type: &str,
    subject_id: &str,
    payload: serde_json::Value,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_kind, actor_id, actor_display_name, event_type, subject_type, subject_id, payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&actor.kind)
    .bind(&actor.id)
    .bind(&actor.display_name)
    .bind(event_type)
    .bind(subject_type)
    .bind(subject_id)
    .bind(payload.to_string())
    .execute(&mut **tx)
    .await
    .context("failed to append audit event")?;
    Ok(())
}

fn agent_run_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT agent_runs.id, agent_runs.task_id, agent_runs.task_queue_id, agent_runs.worker_actor_kind, agent_runs.worker_actor_id, agent_runs.worker_actor_display_name, agent_runs.worker_id, agent_runs.launcher_kind, agent_runs.lease_expires_at, agent_runs.last_heartbeat_at, agent_runs.outcome, agent_runs.failure_reason, agent_runs.failure_reason_code, agent_runs.created_at, agent_runs.finished_at FROM agent_runs {where_clause}"
    )
}

fn validate_transition(task: &Task, to_state: &str, actor: &Actor) -> Result<()> {
    match actor.kind.as_str() {
        "operator" | "review_agent" | "worker_agent" => {}
        _ => anyhow::bail!(
            "State Transitions require an Operator, Review Agent, or Worker Agent actor"
        ),
    }
    if task.state == to_state {
        anyhow::bail!("Task is already in requested Task State");
    }
    let allowed = match task.state.as_str() {
        "backlog" => matches!(to_state, "ready" | "canceled"),
        "ready" => matches!(to_state, "in_progress" | "canceled"),
        "in_progress" => matches!(
            to_state,
            "human_review" | "integrating" | "done" | "canceled"
        ),
        "human_review" => matches!(to_state, "rework" | "integrating" | "canceled"),
        "rework" => matches!(
            to_state,
            "in_progress" | "human_review" | "integrating" | "canceled"
        ),
        "integrating" => matches!(to_state, "done" | "rework" | "canceled"),
        "done" | "canceled" => false,
        _ => false,
    };
    if !allowed {
        anyhow::bail!(
            "State Transition from {} to {to_state} is not allowed",
            task.state
        );
    }
    if task.review_required && to_state == "integrating" && task.state != "human_review" {
        anyhow::bail!(
            "Review-required Tasks must transition through Human Review before Integrating"
        );
    }
    if actor.kind == "worker_agent" {
        if to_state == "integrating" {
            if task.review_required {
                anyhow::bail!(
                    "Worker Agent cannot transition review-required Tasks to Integrating"
                );
            }
        } else if to_state != "human_review" && to_state != "canceled" {
            anyhow::bail!("Worker Agent cannot request this State Transition");
        }
    }
    Ok(())
}

fn requires_completion_gates(to_state: &str) -> bool {
    matches!(to_state, "human_review" | "integrating" | "done")
}

async fn ensure_ready_requirements_exist(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: &str,
) -> Result<()> {
    let criteria_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM acceptance_criteria WHERE task_id = ?")
            .bind(task_id)
            .fetch_one(&mut **tx)
            .await
            .context("failed to count Acceptance Criteria")?;
    let validation_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM validation_items WHERE task_id = ?")
            .bind(task_id)
            .fetch_one(&mut **tx)
            .await
            .context("failed to count Validation Items")?;
    if criteria_count == 0 || validation_count == 0 {
        anyhow::bail!(
            "Ready Tasks require at least one Acceptance Criterion and one Validation Item"
        );
    }
    Ok(())
}

async fn ensure_worker_owns_active_run(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: &str,
    agent_run_id: Option<&str>,
    actor: &Actor,
) -> Result<()> {
    let Some(agent_run_id) = agent_run_id else {
        anyhow::bail!("Worker Agent Integrating transition requires an active Agent Run ID");
    };
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM agent_runs
        WHERE id = ?
          AND task_id = ?
          AND outcome IS NULL
          AND lease_expires_at > CURRENT_TIMESTAMP
          AND worker_actor_kind = ?
          AND worker_actor_id = ?
        "#,
    )
    .bind(agent_run_id)
    .bind(task_id)
    .bind(&actor.kind)
    .bind(&actor.id)
    .fetch_one(&mut **tx)
    .await
    .context("failed to verify active Agent Run ownership")?;
    if count != 1 {
        anyhow::bail!("Worker Agent does not own an active Claim Lease for this Task");
    }
    Ok(())
}

async fn ensure_completion_gates_pass(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: &str,
) -> Result<()> {
    let unsatisfied_criteria: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM acceptance_criteria
        WHERE task_id = ? AND status NOT IN ('satisfied', 'waived')
        "#,
    )
    .bind(task_id)
    .fetch_one(&mut **tx)
    .await
    .context("failed to check Acceptance Criteria gates")?;
    let unpassed_validation: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM validation_items
        WHERE task_id = ? AND status NOT IN ('passed', 'waived')
        "#,
    )
    .bind(task_id)
    .fetch_one(&mut **tx)
    .await
    .context("failed to check Validation Item gates")?;
    if unsatisfied_criteria > 0 || unpassed_validation > 0 {
        anyhow::bail!(
            "State Transition requires all Acceptance Criteria and Validation Items to pass gates"
        );
    }
    Ok(())
}

fn validate_task_queue(input: &CreateTaskQueue) -> Result<()> {
    ensure_not_blank("Task Queue Key", &input.key)?;
    if input.key.contains('/') || input.key.contains('\\') {
        anyhow::bail!("Task Queue Key must not contain path separators");
    }
    ensure_not_blank("Task Queue name", &input.name)?;
    ensure_not_blank(
        "Managed Source Repository",
        &input.managed_source_repository,
    )?;
    ensure_not_blank("Main Branch", &input.main_branch)?;
    ensure_not_blank("Worktree Root", &input.worktree_root)?;
    ensure_not_blank("Branch Template", &input.branch_template)?;
    validate_queue_concurrency_limit(input.queue_concurrency_limit)?;
    Ok(())
}

fn validate_queue_concurrency_limit(limit: Option<i64>) -> Result<()> {
    if let Some(limit) = limit {
        if limit <= 0 {
            anyhow::bail!("Queue Concurrency Limit must be positive");
        }
    }
    Ok(())
}

fn validate_requirement_status(
    input: &UpdateRequirementStatus,
    actor: &Actor,
    allowed_statuses: &[&str],
) -> Result<()> {
    if !allowed_statuses.contains(&input.status.as_str()) {
        anyhow::bail!("invalid requirement status {}", input.status);
    }
    if input.status == "waived" {
        if actor.kind == "worker_agent" {
            anyhow::bail!("Worker Agents cannot create Waivers");
        }
        if actor.kind != "operator" && actor.kind != "review_agent" {
            anyhow::bail!("Waivers require an Operator or Review Agent actor");
        }
        match input.waiver_reason.as_deref() {
            Some(reason) if !reason.trim().is_empty() => {}
            _ => anyhow::bail!("Waivers require an explicit reason"),
        }
    }
    Ok(())
}

pub fn validate_create_task(input: &CreateTask) -> Result<()> {
    ensure_not_blank("title", &input.title)?;
    ensure_not_blank("Task Brief", &input.brief)?;
    validate_priority(&input.priority)?;
    validate_state(&input.state)?;
    if input.state != "backlog" && input.state != "ready" {
        anyhow::bail!("Bootstrap Task Creation only supports Backlog or Ready initial Task States");
    }
    if input.state == "ready"
        && (input.acceptance_criteria.is_empty() || input.validation_items.is_empty())
    {
        anyhow::bail!(
            "Ready Tasks require at least one Acceptance Criterion and one Validation Item"
        );
    }
    for criterion in &input.acceptance_criteria {
        ensure_not_blank("Acceptance Criterion", criterion)?;
    }
    for item in &input.validation_items {
        ensure_not_blank("Validation Item", item)?;
    }
    for hint in &input.conflict_hints {
        ensure_not_blank("Task Conflict Hint", hint)?;
    }
    Ok(())
}

fn validate_actor(actor: &Actor) -> Result<()> {
    ensure_not_blank("Actor kind", &actor.kind)?;
    ensure_not_blank("Actor id", &actor.id)?;
    ensure_not_blank("Actor display name", &actor.display_name)?;
    Ok(())
}

fn validate_child_task_actor(actor: &Actor) -> Result<()> {
    validate_actor(actor)?;
    if actor.kind == "operator" || actor.kind == "delegating_agent" || actor.kind == "worker_agent"
    {
        Ok(())
    } else {
        anyhow::bail!(
            "Child Task creation requires an Operator, Delegating Agent, or Worker Agent actor"
        )
    }
}

fn validate_worker_actor(actor: &Actor) -> Result<()> {
    validate_actor(actor)?;
    if actor.kind != "worker_agent" {
        anyhow::bail!("Agent Run mutations require a Worker Agent actor");
    }
    Ok(())
}

fn validate_operator_actor(actor: &Actor) -> Result<()> {
    validate_actor(actor)?;
    if actor.kind != "operator" {
        anyhow::bail!("recovery commands require an Operator actor");
    }
    Ok(())
}

fn validate_positive_seconds(field: &str, value: i64) -> Result<()> {
    if value <= 0 {
        anyhow::bail!("{field} must be positive");
    }
    Ok(())
}

fn validate_run_outcome(outcome: &str) -> Result<()> {
    match outcome {
        "completed" | "failed" | "canceled" => Ok(()),
        _ => anyhow::bail!("invalid Agent Run outcome {outcome}"),
    }
}

fn failure_reason_code_for_finish(input: &FinishRunInput) -> Result<Option<&str>> {
    let Some(code) = input.failure_reason_code.as_deref() else {
        return Ok((input.outcome == "failed").then_some("agent_run_failed"));
    };
    validate_failure_reason_code(code)?;
    if input.outcome == "completed" {
        anyhow::bail!("completed Agent Runs cannot have a failure reason code");
    }
    Ok(Some(code))
}

fn validate_failure_reason_code(code: &str) -> Result<()> {
    match code {
        "agent_run_failed"
        | "local_worktree_setup_failed"
        | "dirty_managed_source_repository"
        | "repo_operation_lock_held"
        | "migration_incompatible"
        | "stale_validation_base"
        | "launcher_start_failed"
        | "launcher_rpc_io_failed"
        | "launcher_exited"
        | "launcher_timeout"
        | "unattended_question"
        | "agent_gated_integration_failed"
        | "operator_failed"
        | "claim_lease_expired"
        | "task_canceled" => Ok(()),
        _ => anyhow::bail!("invalid Agent Run failure reason code {code}"),
    }
}

fn validate_priority(priority: &str) -> Result<()> {
    match priority {
        "urgent" | "high" | "normal" | "low" => Ok(()),
        _ => anyhow::bail!("invalid Priority {priority}"),
    }
}

fn validate_state(state: &str) -> Result<()> {
    match state {
        "backlog" | "ready" | "in_progress" | "human_review" | "rework" | "integrating"
        | "done" | "canceled" => Ok(()),
        _ => anyhow::bail!("invalid Task State {state}"),
    }
}

fn ensure_not_blank(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("{field} must not be blank");
    }
    Ok(())
}

fn normalized_tags(tags: &[String]) -> Vec<String> {
    let mut tags = tags
        .iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    tags.sort();
    tags.dedup();
    tags
}

fn normalized_conflict_hints(hints: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for hint in hints {
        let hint = hint.trim().to_string();
        if !hint.is_empty() && !normalized.contains(&hint) {
            normalized.push(hint);
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use sqlx::Row;

    use super::*;

    #[tokio::test]
    async fn migrations_are_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = connect(&db_path).await.expect("connect");

        run_migrations(&pool).await.expect("first migrate");
        run_migrations(&pool).await.expect("second migrate");

        let row = sqlx::query("select value from tasker_metadata where key = 'schema_version'")
            .fetch_one(&pool)
            .await
            .expect("metadata row");
        let value: String = row.get("value");

        assert_eq!(value, "1");
    }

    #[tokio::test]
    async fn sqlite_write_retry_retries_busy_errors_with_bounded_backoff() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
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
        let db_path = temp.path().join("tasker.db");
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
        let first = sample_queue("TASK", "Tasker");
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
        let input = sample_queue("TASK", "Tasker");

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
        let input = sample_queue("TASK", "Tasker");

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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
    async fn backlog_task_may_be_created_before_requirements_are_complete() {
        let (_temp, pool) = migrated_pool().await;
        create_task_queue(
            &pool,
            &sample_queue("TASK", "Tasker"),
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
    async fn mutations_require_attributed_actor() {
        let (_temp, pool) = migrated_pool().await;
        let error = create_task_queue(
            &pool,
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
    }

    #[tokio::test]
    async fn creates_child_tasks_in_parent_queue_and_records_relationships() {
        let (_temp, pool) = migrated_pool().await;
        create_task_queue(
            &pool,
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
                target: "tasker/TASK-1".to_string(),
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
            &sample_queue("TASK", "Tasker"),
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
        let second =
            update_workpad_note(&pool, "TASK-1", "second note", &Actor::operator("tester"))
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            },
            &worker_actor(),
        )
        .await
        .expect_err("worker cannot integrate review required");
        assert!(forbidden.to_string().contains("Review-required"));
    }

    #[tokio::test]
    async fn updates_requirement_statuses_and_audit_events() {
        let (_temp, pool) = migrated_pool().await;
        create_task_queue(
            &pool,
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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

        let detail = update_acceptance_criterion_status(
            &pool,
            "TASK-1",
            1,
            &waiver,
            &Actor::operator("tester"),
        )
        .await
        .expect("operator waiver succeeds");
        assert_eq!(detail.acceptance_criteria[0].status, "waived");
        assert_eq!(
            detail.acceptance_criteria[0].waiver_reason.as_deref(),
            Some("not needed")
        );
    }

    #[tokio::test]
    async fn claim_next_claims_ready_task_and_moves_to_in_progress() {
        let (_temp, pool) = migrated_pool().await;
        create_task_queue(
            &pool,
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
        let mut queue = sample_queue("TASK", "Tasker");
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
                    r#"{"usage":{"input_tokens":11,"output_tokens":7,"total_tokens":18}}"#
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
            &sample_queue("TASK", "Tasker"),
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
                "stdout":"{\"role\":\"user\",\"content\":\"brief\"}\n{\"role\":\"assistant\",\"content\":\"plan\",\"usage\":{\"input_tokens\":30,\"output_tokens\":12,\"total_tokens\":42}}\n{\"type\":\"message_update\",\"assistantMessageEvent\":{\"partial\":{\"usage\":{\"input\":45,\"output\":15,\"totalTokens\":60,\"cacheRead\":1000,\"cacheWrite\":50}}}}\n{\"type\":\"tool_call\",\"tool_name\":\"read\",\"args\":{\"path\":\"src/lib.rs\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"read\",\"args\":{\"path\":\"src/lib.rs\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"bin/tasker-local task show TASK-1\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"bin/tasker-local task show TASK-1\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"tasker status\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"tasker status\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"edit\",\"args\":{}}\n{\"type\":\"tool_call\",\"tool_name\":\"write\",\"args\":{}}\n{\"type\":\"tool_call\",\"tool_name\":\"tasker.get_task_context\",\"args\":{}}\n{\"type\":\"message_update\",\"message\":{\"content\":[{\"type\":\"toolCall\",\"id\":\"call-read-1\",\"name\":\"read\",\"partialJson\":\"\"},{\"type\":\"toolCall\",\"id\":\"call-read-1\",\"name\":\"read\",\"partialJson\":\"{\\\"path\\\":\\\"CONTEXT.md\\\"}\"},{\"type\":\"toolCall\",\"id\":\"call-read-2\",\"name\":\"read\",\"partialJson\":\"{\\\"path\\\":\\\"CONTEXT.md\\\"}\"}]}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"cargo test -p tasker-db\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"git status --short\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"rg telemetry crates\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"ls crates\"}}\n{\"type\":\"tool_call\",\"tool_name\":\"bash\",\"args\":{\"command\":\"bun test\"}}\n{\"type\":\"tool_error\",\"tool_name\":\"read\",\"args\":{\"path\":\"src/lib.rs\"},\"status\":\"error\"}\n{\"type\":\"tool_error\",\"tool_name\":\"read\",\"args\":{\"path\":\"src/lib.rs\"},\"status\":\"error\"}\n{\"type\":\"agent_end\"}\n",
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
        assert_eq!(metrics.launcher_kind, "pi");
        assert_eq!(metrics.exit_code, Some(0));
        assert_eq!(metrics.timed_out, Some(0));
        assert_eq!(metrics.unattended_question_detected, Some(0));
        assert_eq!(metrics.transcript_jsonl_event_count, Some(1));
        assert_eq!(metrics.tool_call_count, Some(16));
        assert_eq!(metrics.tool_error_count, Some(2));
        assert_eq!(metrics.repeated_failed_tool_attempt_count, Some(1));
        assert_eq!(metrics.repeated_read_count, Some(2));
        assert_eq!(metrics.repeated_tasker_context_fetch_count, Some(2));
        let tool_counts: std::collections::BTreeMap<String, i64> =
            serde_json::from_str(&metrics.tool_call_counts_json).expect("tool counts");
        assert_eq!(tool_counts.get("read"), Some(&4));
        assert_eq!(tool_counts.get("bash"), Some(&9));
        assert_eq!(tool_counts.get("edit"), Some(&1));
        assert_eq!(tool_counts.get("write"), Some(&1));
        assert_eq!(tool_counts.get("tasker.get_task_context"), Some(&1));
        let shell_counts: std::collections::BTreeMap<String, i64> =
            serde_json::from_str(&metrics.shell_command_counts_json).expect("shell counts");
        assert_eq!(shell_counts.get("tasker_cli"), Some(&4));
        assert_eq!(shell_counts.get("cargo"), Some(&1));
        assert_eq!(shell_counts.get("git"), Some(&1));
        assert_eq!(shell_counts.get("search"), Some(&1));
        assert_eq!(shell_counts.get("filesystem"), Some(&1));
        assert_eq!(shell_counts.get("package_build"), Some(&1));
        assert_eq!(metrics.assistant_turn_count, Some(1));
        assert_eq!(metrics.user_turn_count, Some(1));
        assert_eq!(metrics.input_tokens, Some(45));
        assert_eq!(metrics.output_tokens, Some(15));
        assert_eq!(metrics.total_tokens, Some(60));
        assert_eq!(metrics.cache_read_tokens, Some(1000));
        assert_eq!(metrics.cache_write_tokens, Some(50));
        assert_eq!(metrics.max_context_tokens, Some(60));
        let hints: Vec<String> =
            serde_json::from_str(&metrics.efficiency_hints_json).expect("hints");
        assert!(hints
            .iter()
            .any(|hint| hint == "repeated failed tool attempts"));
        assert!(hints.iter().any(|hint| hint == "repeated file reads"));
        assert!(hints
            .iter()
            .any(|hint| hint == "repeated Tasker context fetches"));
    }

    #[tokio::test]
    async fn agent_run_metrics_warn_for_missing_and_malformed_transcripts() {
        let (temp, pool) = migrated_pool().await;
        create_task_queue(
            &pool,
            &sample_queue("TASK", "Tasker"),
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
            &sample_queue("TASK", "Tasker"),
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
        let mut queue = sample_queue("TASK", "Tasker");
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
        create_task_queue(&pool, &sample_queue("TASK", "Tasker"), &actor)
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
        create_task_queue(&pool, &sample_queue("TASK", "Tasker"), &actor)
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
        let db_path = temp.path().join("tasker.db");
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
        let db_path = temp.path().join("tasker.db");
        let pool = connect(&db_path).await.expect("connect");
        run_migrations(&pool).await.expect("migrate");
        (temp, pool)
    }

    fn sample_queue(key: &str, name: &str) -> CreateTaskQueue {
        CreateTaskQueue {
            key: key.to_string(),
            name: name.to_string(),
            managed_source_repository: "/repo/tasker".to_string(),
            main_branch: "main".to_string(),
            worktree_root: "/worktrees".to_string(),
            branch_template: "tasker/{task_identifier}".to_string(),
            done_worktree_retention: false,
            queue_concurrency_limit: None,
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
        }
    }
}

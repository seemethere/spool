use std::{future::Future, path::Path};

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
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .context("failed to run SQLite migrations")
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
    pub task_state: String,
    pub agent_run_id: String,
    pub launcher_kind: String,
    pub worker_id: String,
    pub lease_expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct ActiveRetryHoldStatus {
    pub queue_key: String,
    pub task_identifier: String,
    pub hold_until: String,
    pub reason: String,
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
    pub retry_hold_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorFailRunInput {
    pub failure_reason: String,
    pub retry_hold_seconds: Option<i64>,
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
               tasks.review_required, tasks.created_at, tasks.updated_at
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

    Ok(Some(TaskDetail {
        task,
        acceptance_criteria,
        validation_items,
        tags,
        workpad_note,
        task_links,
        conflict_hints,
        conflict_overlaps,
    }))
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
            SET outcome = 'canceled', finished_at = CURRENT_TIMESTAMP, failure_reason = 'Task canceled'
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

    let payload_json = serde_json::json!({
        "identifier": identifier,
        "position": position,
        "previous_status": previous_status,
        "status": input.status,
        "waiver_reason": waiver_reason,
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

pub async fn active_retry_holds_for_status(
    pool: &SqlitePool,
) -> Result<Vec<ActiveRetryHoldStatus>> {
    sqlx::query_as::<_, ActiveRetryHoldStatus>(
        r#"
        SELECT
            task_queues.key AS queue_key,
            tasks.identifier AS task_identifier,
            task_retry_holds.hold_until AS hold_until,
            task_retry_holds.reason AS reason
        FROM task_retry_holds
        JOIN tasks ON tasks.id = task_retry_holds.task_id
        JOIN task_queues ON task_queues.id = tasks.task_queue_id
        WHERE task_retry_holds.hold_until > CURRENT_TIMESTAMP
        ORDER BY task_queues.key, tasks.identifier
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to load active Retry Holds for status")
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
               tasks.review_required, tasks.created_at, tasks.updated_at
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
                  last_heartbeat_at, outcome, failure_reason, created_at, finished_at
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

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let run = sqlx::query_as::<_, AgentRun>(
        r#"
        UPDATE agent_runs
        SET outcome = ?, failure_reason = ?, finished_at = CURRENT_TIMESTAMP
        WHERE id = ?
          AND outcome IS NULL
          AND lease_expires_at > CURRENT_TIMESTAMP
          AND worker_actor_kind = ?
          AND worker_actor_id = ?
        RETURNING id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                  worker_actor_display_name, worker_id, launcher_kind, lease_expires_at,
                  last_heartbeat_at, outcome, failure_reason, created_at, finished_at
        "#,
    )
    .bind(&input.outcome)
    .bind(&input.failure_reason)
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
            "retry_hold_seconds": input.retry_hold_seconds,
        }),
    )
    .await?;
    tx.commit().await.context("failed to commit transaction")?;
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

    let mut tx = pool.begin().await.context("failed to begin transaction")?;
    let run = sqlx::query_as::<_, AgentRun>(
        r#"
        UPDATE agent_runs
        SET outcome = 'failed', failure_reason = ?, finished_at = CURRENT_TIMESTAMP
        WHERE id = ? AND outcome IS NULL
        RETURNING id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                  worker_actor_display_name, worker_id, launcher_kind, lease_expires_at,
                  last_heartbeat_at, outcome, failure_reason, created_at, finished_at
        "#,
    )
    .bind(input.failure_reason.trim())
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
            "retry_hold_seconds": seconds,
        }),
    )
    .await?;

    tx.commit().await.context("failed to commit transaction")?;
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
               tasks.state, tasks.review_required, tasks.created_at, tasks.updated_at
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
    Ok(AgentRunDetail {
        run,
        task,
        launcher_session_data,
    })
}

async fn expire_stale_agent_runs(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<()> {
    let expired = sqlx::query_as::<_, AgentRun>(
        r#"
        UPDATE agent_runs
        SET outcome = 'expired', finished_at = CURRENT_TIMESTAMP, failure_reason = 'Claim Lease expired'
        WHERE outcome IS NULL AND lease_expires_at <= CURRENT_TIMESTAMP
        RETURNING id, task_id, task_queue_id, worker_actor_kind, worker_actor_id,
                  worker_actor_display_name, worker_id, launcher_kind, lease_expires_at,
                  last_heartbeat_at, outcome, failure_reason, created_at, finished_at
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
            }),
        )
        .await?;
        append_audit_event_in_tx(
            tx,
            &actor,
            "agent_run.expired",
            "agent_run",
            &run.id,
            serde_json::json!({ "reason": "Claim Lease expired" }),
        )
        .await?;
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
        "SELECT agent_runs.id, agent_runs.task_id, agent_runs.task_queue_id, agent_runs.worker_actor_kind, agent_runs.worker_actor_id, agent_runs.worker_actor_display_name, agent_runs.worker_id, agent_runs.launcher_kind, agent_runs.lease_expires_at, agent_runs.last_heartbeat_at, agent_runs.outcome, agent_runs.failure_reason, agent_runs.created_at, agent_runs.finished_at FROM agent_runs {where_clause}"
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
        finish_run(
            &pool,
            &claimed.run.id,
            &FinishRunInput {
                outcome: "failed".to_string(),
                failure_reason: Some("fake failure".to_string()),
                retry_hold_seconds: Some(60),
            },
            &worker_actor(),
        )
        .await
        .expect("finish failed");

        assert!(claim_next(&pool, &sample_claim("TASK"), &worker_actor())
            .await
            .expect("claim")
            .is_none());
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

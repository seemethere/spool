use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    FromRow, SqlitePool,
};
use uuid::Uuid;

pub const LOCAL_TOKEN_NAME: &str = "local";

pub fn sqlite_url(db_path: &Path) -> String {
    format!("sqlite://{}", db_path.display())
}

pub async fn connect(db_path: &Path) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .foreign_keys(true);

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

pub async fn ensure_local_api_token(pool: &SqlitePool) -> Result<String> {
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
    pub created_at: String,
    pub updated_at: String,
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
    validate_actor(actor)?;
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
            done_worktree_retention
        ) VALUES (?, ?, ?, 'local_worktree', ?, ?, ?, ?, ?)
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskDetail {
    pub task: Task,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub validation_items: Vec<ValidationItem>,
    pub tags: Vec<String>,
    pub workpad_note: Option<WorkpadNote>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct QueueStatus {
    pub queue_key: String,
    pub queue_name: String,
    pub state: String,
    pub task_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateRequirementStatus {
    pub status: String,
    pub waiver_reason: Option<String>,
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
               worktree_root, branch_template, done_worktree_retention, created_at, updated_at
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

    Ok(Some(TaskDetail {
        task,
        acceptance_criteria,
        validation_items,
        tags,
        workpad_note,
    }))
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
            COALESCE(tasks.state, 'none') AS state,
            COUNT(tasks.id) AS task_count
        FROM task_queues
        LEFT JOIN tasks ON tasks.task_queue_id = task_queues.id
        GROUP BY task_queues.key, task_queues.name, tasks.state
        ORDER BY task_queues.key, tasks.state
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to load Tasker status")
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
    Ok(())
}

fn validate_actor(actor: &Actor) -> Result<()> {
    ensure_not_blank("Actor kind", &actor.kind)?;
    ensure_not_blank("Actor id", &actor.id)?;
    ensure_not_blank("Actor display name", &actor.display_name)?;
    Ok(())
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

#[cfg(test)]
mod tests {
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
            .any(|row| row.queue_key == "TASK" && row.state == "ready" && row.task_count == 1));
        assert!(status
            .iter()
            .any(|row| row.queue_key == "TASK" && row.state == "backlog" && row.task_count == 1));
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
        }
    }
}

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    FromRow, SqlitePool,
};
use uuid::Uuid;

pub fn sqlite_url(db_path: &Path) -> String {
    format!("sqlite://{}", db_path.display())
}

pub async fn connect(db_path: &Path) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true);

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
}

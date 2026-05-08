use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct ServerState {
    pub version: String,
    pub pool: SqlitePool,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct VersionResponse {
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateQueueRequest {
    pub actor: tasker_db::Actor,
    pub queue: tasker_db::CreateTaskQueue,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

pub fn router(app_version: impl Into<String>, pool: SqlitePool) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/version", get(version))
        .route("/queues", post(create_queue).get(list_queues))
        .route("/queues/{key}", get(get_queue))
        .with_state(ServerState {
            version: app_version.into(),
            pool,
        })
}

pub async fn serve(addr: SocketAddr, version: impl Into<String>, pool: SqlitePool) -> Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind Tasker Service to {addr}"))?;
    axum::serve(listener, router(version, pool))
        .await
        .context("Tasker Service failed")
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn version(State(state): State<ServerState>) -> Json<VersionResponse> {
    Json(VersionResponse {
        version: state.version,
    })
}

async fn create_queue(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<CreateQueueRequest>,
) -> Result<(StatusCode, Json<tasker_db::TaskQueue>), (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    require_operator(&request.actor)?;

    if tasker_db::get_task_queue(&state.pool, &request.queue.key)
        .await
        .map_err(internal_error)?
        .is_some()
    {
        return Err(error_response(
            StatusCode::CONFLICT,
            format!("Task Queue {} already exists", request.queue.key),
        ));
    }

    tasker_db::create_task_queue(&state.pool, &request.queue, &request.actor)
        .await
        .map(|queue| (StatusCode::CREATED, Json(queue)))
        .map_err(queue_create_error)
}

async fn list_queues(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<tasker_db::TaskQueue>>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    tasker_db::list_task_queues(&state.pool)
        .await
        .map(Json)
        .map_err(internal_error)
}

async fn get_queue(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Json<tasker_db::TaskQueue>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    match tasker_db::get_task_queue(&state.pool, &key)
        .await
        .map_err(internal_error)?
    {
        Some(queue) => Ok(Json(queue)),
        None => Err(error_response(
            StatusCode::NOT_FOUND,
            format!("Task Queue {key} not found"),
        )),
    }
}

async fn require_auth(
    pool: &SqlitePool,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let Some(header) = headers.get(axum::http::header::AUTHORIZATION) else {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "missing bearer token",
        ));
    };
    let header = header.to_str().map_err(|_| {
        error_response(
            StatusCode::UNAUTHORIZED,
            "authorization header is not valid UTF-8",
        )
    })?;
    let Some(token) = header.strip_prefix("Bearer ") else {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "authorization header must use Bearer token",
        ));
    };

    if tasker_db::authenticate_api_token(pool, token)
        .await
        .map_err(internal_error)?
    {
        Ok(())
    } else {
        Err(error_response(
            StatusCode::UNAUTHORIZED,
            "invalid bearer token",
        ))
    }
}

fn require_operator(actor: &tasker_db::Actor) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if actor.kind == "operator" {
        Ok(())
    } else {
        Err(error_response(
            StatusCode::FORBIDDEN,
            "Task Queue creation requires an Operator actor",
        ))
    }
}

fn queue_create_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    if is_unique_queue_key_error(&error) {
        error_response(StatusCode::CONFLICT, "Task Queue already exists")
    } else {
        internal_error(error)
    }
}

fn is_unique_queue_key_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().contains("task_queues.key"))
}

fn internal_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    error_response(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn error_response(
    status: StatusCode,
    error: impl Into<String>,
) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: error.into(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn health_returns_ok_json() {
        let (_temp, pool, _token) = migrated_pool().await;
        let response = router("test-version", pool)
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!({ "status": "ok" }));
    }

    #[tokio::test]
    async fn version_returns_configured_version() {
        let (_temp, pool, _token) = migrated_pool().await;
        let response = router("0.1.0-test", pool)
            .oneshot(
                Request::builder()
                    .uri("/version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!({ "version": "0.1.0-test" }));
    }

    #[tokio::test]
    async fn queue_endpoints_create_show_and_list() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool);

        let response = app
            .clone()
            .oneshot(create_queue_request("TASK", "Tasker", "operator", &token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = app
            .clone()
            .oneshot(authorized_get("/queues/TASK", &token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["key"], "TASK");

        let response = app
            .oneshot(authorized_get("/queues", &token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn queue_endpoints_require_bearer_auth() {
        let (_temp, pool, _token) = migrated_pool().await;
        let response = router("test-version", pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/queues")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "actor": { "kind": "operator", "id": "tester", "display_name": "tester" },
                            "queue": sample_queue_json("TASK", "Tasker")
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn queue_creation_requires_operator_actor() {
        let (_temp, pool, token) = migrated_pool().await;
        let response = router("test-version", pool)
            .oneshot(create_queue_request("TASK", "Tasker", "worker", &token))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn duplicate_queue_create_returns_conflict() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool);

        let first = app
            .clone()
            .oneshot(create_queue_request("TASK", "Tasker", "operator", &token))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);

        let duplicate = app
            .oneshot(create_queue_request("TASK", "Tasker", "operator", &token))
            .await
            .unwrap();
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn missing_queue_returns_not_found() {
        let (_temp, pool, token) = migrated_pool().await;
        let response = router("test-version", pool)
            .oneshot(authorized_get("/queues/MISSING", &token))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    async fn migrated_pool() -> (tempfile::TempDir, SqlitePool, String) {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("tasker.db");
        let pool = tasker_db::connect(&db_path).await.expect("connect");
        tasker_db::run_migrations(&pool).await.expect("migrate");
        let token = tasker_db::ensure_local_api_token(&pool)
            .await
            .expect("local token");
        (temp, pool, token)
    }

    fn create_queue_request(key: &str, name: &str, actor_kind: &str, token: &str) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": actor_kind,
                "id": "tester",
                "display_name": "tester"
            },
            "queue": sample_queue_json(key, name)
        });

        Request::builder()
            .method("POST")
            .uri("/queues")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn authorized_get(uri: &str, token: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    fn sample_queue_json(key: &str, name: &str) -> Value {
        serde_json::json!({
            "key": key,
            "name": name,
            "managed_source_repository": "/repo/tasker",
            "main_branch": "main",
            "worktree_root": "/worktrees",
            "branch_template": "tasker/{task_identifier}",
            "done_worktree_retention": false
        })
    }
}

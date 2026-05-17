use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
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
    pub actor: spool_db::Actor,
    pub queue: spool_db::CreateTaskQueue,
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub actor: spool_db::Actor,
    pub task: spool_db::CreateTask,
}

#[derive(Debug, Deserialize)]
pub struct CreateDelegatedRootTaskRequest {
    pub actor: spool_db::Actor,
    pub draft: spool_db::DelegationTaskDraft,
}

#[derive(Debug, Deserialize)]
pub struct CreateChildTaskRequest {
    pub actor: spool_db::Actor,
    pub task: spool_db::CreateChildTask,
}

#[derive(Debug, Deserialize)]
pub struct RefineBacklogTaskRequest {
    pub actor: spool_db::Actor,
    pub refinement: spool_db::RefineBacklogTask,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWorkpadRequest {
    pub actor: spool_db::Actor,
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct UpsertTaskLinkRequest {
    pub actor: spool_db::Actor,
    pub link: spool_db::UpsertTaskLink,
}

#[derive(Debug, Deserialize)]
pub struct BlockingTaskRelationshipRequest {
    pub actor: spool_db::Actor,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRequirementStatusRequest {
    pub actor: spool_db::Actor,
    pub status: String,
    pub waiver_reason: Option<String>,
    pub validated_base_commit: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TransitionTaskRequest {
    pub actor: spool_db::Actor,
    pub to_state: String,
    pub agent_run_id: Option<String>,
    #[serde(default)]
    pub repair_override: bool,
}

#[derive(Debug, Deserialize)]
pub struct RecordReviewDecisionRequest {
    pub actor: spool_db::Actor,
    pub decision: String,
    pub feedback: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ClaimNextRequest {
    pub actor: spool_db::Actor,
    pub worker_id: String,
    pub launcher_kind: Option<String>,
    pub lease_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatRunRequest {
    pub actor: spool_db::Actor,
    pub lease_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct FinishRunRequest {
    pub actor: spool_db::Actor,
    pub outcome: String,
    pub failure_reason: Option<String>,
    pub failure_reason_code: Option<String>,
    pub retry_hold_seconds: Option<i64>,
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
        .route("/queues/{key}/claim-next", post(claim_next))
        .route("/tasks/bootstrap", post(create_task))
        .route("/tasks/delegated-root", post(create_delegated_root_task))
        .route(
            "/tasks/{identifier}",
            get(get_task).post(post_task_identifier),
        )
        .route("/tasks/{identifier}/refine", post(refine_backlog_task))
        .route(
            "/tasks/{identifier}/context-bundle",
            get(get_task_context_bundle),
        )
        .route("/tasks/{identifier}/child-tasks", post(create_child_task))
        .route(
            "/tasks/{identifier}/blocking-tasks/{blocking_identifier}",
            post(add_blocking_task_relationship).delete(remove_blocking_task_relationship),
        )
        .route(
            "/tasks/{identifier}/workpad",
            axum::routing::put(update_workpad),
        )
        .route("/tasks/{identifier}/links", post(upsert_task_link))
        .route("/tasks/{identifier}/transition", post(transition_task))
        .route(
            "/tasks/{identifier}/review-decision",
            post(record_review_decision),
        )
        .route(
            "/tasks/{identifier}/acceptance-criteria/{position}/status",
            axum::routing::put(update_acceptance_criterion_status),
        )
        .route(
            "/tasks/{identifier}/validation-items/{position}/status",
            axum::routing::put(update_validation_item_status),
        )
        .route("/tasks/{identifier}/audit-events", get(task_audit_events))
        .route("/agent-runs/{run_id}", get(get_agent_run))
        .route("/agent-runs/{run_id}/heartbeat", post(heartbeat_run))
        .route("/agent-runs/{run_id}/finish", post(finish_run))
        .route("/status", get(status))
        .with_state(ServerState {
            version: app_version.into(),
            pool,
        })
}

pub async fn serve(addr: SocketAddr, version: impl Into<String>, pool: SqlitePool) -> Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind Spool Service to {addr}"))?;
    axum::serve(listener, router(version, pool))
        .await
        .context("Spool Service failed")
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
) -> Result<(StatusCode, Json<spool_db::TaskQueue>), (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    require_operator(&request.actor)?;

    if spool_db::get_task_queue(&state.pool, &request.queue.key)
        .await
        .map_err(internal_error)?
        .is_some()
    {
        return Err(error_response(
            StatusCode::CONFLICT,
            format!("Task Queue {} already exists", request.queue.key),
        ));
    }

    spool_db::create_task_queue(&state.pool, &request.queue, &request.actor)
        .await
        .map(|queue| (StatusCode::CREATED, Json(queue)))
        .map_err(queue_create_error)
}

async fn create_task(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<spool_db::TaskDetail>), (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    require_task_create_actor(&request.actor)?;
    spool_db::create_task(&state.pool, &request.task, &request.actor)
        .await
        .map(|task| (StatusCode::CREATED, Json(task)))
        .map_err(task_mutation_error)
}

async fn create_delegated_root_task(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<CreateDelegatedRootTaskRequest>,
) -> Result<(StatusCode, Json<spool_db::TaskDetail>), (StatusCode, Json<ErrorResponse>)> {
    create_delegated_root_task_with_request(state, headers, request).await
}

async fn post_task_identifier(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
    Json(request): Json<CreateDelegatedRootTaskRequest>,
) -> Result<(StatusCode, Json<spool_db::TaskDetail>), (StatusCode, Json<ErrorResponse>)> {
    if identifier != "delegated-root" {
        return Err(error_response(
            StatusCode::METHOD_NOT_ALLOWED,
            format!("POST /tasks/{identifier} is not supported"),
        ));
    }
    create_delegated_root_task_with_request(state, headers, request).await
}

async fn create_delegated_root_task_with_request(
    state: ServerState,
    headers: HeaderMap,
    request: CreateDelegatedRootTaskRequest,
) -> Result<(StatusCode, Json<spool_db::TaskDetail>), (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    spool_db::create_delegated_root_task(&state.pool, &request.draft, &request.actor)
        .await
        .map(|task| (StatusCode::CREATED, Json(task)))
        .map_err(task_mutation_error)
}

async fn create_child_task(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
    Json(request): Json<CreateChildTaskRequest>,
) -> Result<(StatusCode, Json<spool_db::TaskDetail>), (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    require_child_task_actor(&request.actor)?;
    spool_db::create_child_task(&state.pool, &identifier, &request.task, &request.actor)
        .await
        .map(|task| (StatusCode::CREATED, Json(task)))
        .map_err(task_mutation_error)
}

async fn add_blocking_task_relationship(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((identifier, blocking_identifier)): Path<(String, String)>,
    Json(request): Json<BlockingTaskRelationshipRequest>,
) -> Result<Json<spool_db::TaskDetail>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    require_operator(&request.actor)?;
    spool_db::add_blocking_task_relationship(
        &state.pool,
        &identifier,
        &blocking_identifier,
        &request.actor,
    )
    .await
    .map(Json)
    .map_err(task_mutation_error)
}

async fn remove_blocking_task_relationship(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((identifier, blocking_identifier)): Path<(String, String)>,
    Json(request): Json<BlockingTaskRelationshipRequest>,
) -> Result<Json<spool_db::TaskDetail>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    require_operator(&request.actor)?;
    spool_db::remove_blocking_task_relationship(
        &state.pool,
        &identifier,
        &blocking_identifier,
        &request.actor,
    )
    .await
    .map(Json)
    .map_err(task_mutation_error)
}

async fn refine_backlog_task(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
    Json(request): Json<RefineBacklogTaskRequest>,
) -> Result<Json<spool_db::TaskDetail>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    spool_db::refine_backlog_task(
        &state.pool,
        &identifier,
        &request.refinement,
        &request.actor,
    )
    .await
    .map(Json)
    .map_err(task_mutation_error)
}

async fn get_task(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
) -> Result<Json<spool_db::TaskDetail>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    match spool_db::get_task_detail(&state.pool, &identifier)
        .await
        .map_err(internal_error)?
    {
        Some(task) => Ok(Json(task)),
        None => Err(error_response(
            StatusCode::NOT_FOUND,
            format!("Task {identifier} not found"),
        )),
    }
}

async fn get_task_context_bundle(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
) -> Result<Json<spool_db::TaskContextBundle>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    match spool_db::get_task_context_bundle(&state.pool, &identifier)
        .await
        .map_err(internal_error)?
    {
        Some(bundle) => Ok(Json(bundle)),
        None => Err(error_response(
            StatusCode::NOT_FOUND,
            format!("Task {identifier} not found"),
        )),
    }
}

async fn transition_task(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
    Json(request): Json<TransitionTaskRequest>,
) -> Result<Json<spool_db::TaskDetail>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    spool_db::transition_task_state(
        &state.pool,
        &identifier,
        &spool_db::TransitionTaskState {
            to_state: request.to_state,
            agent_run_id: request.agent_run_id,
            repair_override: request.repair_override,
        },
        &request.actor,
    )
    .await
    .map(Json)
    .map_err(task_mutation_error)
}

async fn record_review_decision(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
    Json(request): Json<RecordReviewDecisionRequest>,
) -> Result<Json<spool_db::TaskDetail>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    spool_db::record_review_decision(
        &state.pool,
        &identifier,
        &spool_db::RecordReviewDecision {
            decision: request.decision,
            feedback: request.feedback,
        },
        &request.actor,
    )
    .await
    .map(Json)
    .map_err(task_mutation_error)
}

async fn update_acceptance_criterion_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((identifier, position)): Path<(String, i64)>,
    Json(request): Json<UpdateRequirementStatusRequest>,
) -> Result<Json<spool_db::TaskDetail>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    let input = spool_db::UpdateRequirementStatus {
        status: request.status,
        waiver_reason: request.waiver_reason,
        validated_base_commit: request.validated_base_commit,
    };
    spool_db::update_acceptance_criterion_status(
        &state.pool,
        &identifier,
        position,
        &input,
        &request.actor,
    )
    .await
    .map(Json)
    .map_err(task_mutation_error)
}

async fn update_validation_item_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((identifier, position)): Path<(String, i64)>,
    Json(request): Json<UpdateRequirementStatusRequest>,
) -> Result<Json<spool_db::TaskDetail>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    let input = spool_db::UpdateRequirementStatus {
        status: request.status,
        waiver_reason: request.waiver_reason,
        validated_base_commit: request.validated_base_commit,
    };
    spool_db::update_validation_item_status(
        &state.pool,
        &identifier,
        position,
        &input,
        &request.actor,
    )
    .await
    .map(Json)
    .map_err(task_mutation_error)
}

async fn task_audit_events(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
) -> Result<Json<Vec<spool_db::AuditEvent>>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    spool_db::list_task_audit_events(&state.pool, &identifier)
        .await
        .map(Json)
        .map_err(task_mutation_error)
}

async fn update_workpad(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
    Json(request): Json<UpdateWorkpadRequest>,
) -> Result<Json<spool_db::TaskDetail>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    require_workpad_actor(&request.actor)?;
    spool_db::update_workpad_note(&state.pool, &identifier, &request.body, &request.actor)
        .await
        .map(Json)
        .map_err(task_mutation_error)
}

async fn upsert_task_link(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
    Json(request): Json<UpsertTaskLinkRequest>,
) -> Result<Json<spool_db::TaskDetail>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    require_task_link_actor(&request.actor)?;
    spool_db::upsert_task_link(&state.pool, &identifier, &request.link, &request.actor)
        .await
        .map(Json)
        .map_err(task_mutation_error)
}

async fn status(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<spool_db::QueueStatus>>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    spool_db::status_by_queue_and_state(&state.pool)
        .await
        .map(Json)
        .map_err(internal_error)
}

async fn claim_next(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Json(request): Json<ClaimNextRequest>,
) -> Result<axum::response::Response, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    let input = spool_db::ClaimNextInput {
        queue_key: key,
        worker_id: request.worker_id,
        launcher_kind: request.launcher_kind.unwrap_or_else(|| "fake".to_string()),
        lease_seconds: request.lease_seconds.unwrap_or(90),
    };
    match spool_db::claim_next(&state.pool, &input, &request.actor)
        .await
        .map_err(task_mutation_error)?
    {
        Some(claimed) => Ok(Json(claimed).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

async fn get_agent_run(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<spool_db::AgentRunDetail>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    spool_db::get_agent_run_detail(&state.pool, &run_id)
        .await
        .map_err(internal_error)?
        .map(Json)
        .ok_or_else(|| {
            error_response(
                StatusCode::NOT_FOUND,
                format!("Agent Run {run_id} not found"),
            )
        })
}

async fn heartbeat_run(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
    Json(request): Json<HeartbeatRunRequest>,
) -> Result<Json<spool_db::AgentRun>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    spool_db::heartbeat_run(
        &state.pool,
        &run_id,
        request.lease_seconds.unwrap_or(90),
        &request.actor,
    )
    .await
    .map(Json)
    .map_err(task_mutation_error)
}

async fn finish_run(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
    Json(request): Json<FinishRunRequest>,
) -> Result<Json<spool_db::AgentRun>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    spool_db::finish_run(
        &state.pool,
        &run_id,
        &spool_db::FinishRunInput {
            outcome: request.outcome,
            failure_reason: request.failure_reason,
            failure_reason_code: request.failure_reason_code,
            retry_hold_seconds: request.retry_hold_seconds,
        },
        &request.actor,
    )
    .await
    .map(Json)
    .map_err(task_mutation_error)
}

async fn list_queues(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<spool_db::TaskQueue>>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    spool_db::list_task_queues(&state.pool)
        .await
        .map(Json)
        .map_err(internal_error)
}

async fn get_queue(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Json<spool_db::TaskQueue>, (StatusCode, Json<ErrorResponse>)> {
    require_auth(&state.pool, &headers).await?;
    match spool_db::get_task_queue(&state.pool, &key)
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

    if spool_db::authenticate_api_token(pool, token)
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

fn require_operator(actor: &spool_db::Actor) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if actor.kind == "operator" {
        Ok(())
    } else {
        Err(error_response(
            StatusCode::FORBIDDEN,
            "Task Queue creation requires an Operator actor",
        ))
    }
}

fn require_task_create_actor(
    actor: &spool_db::Actor,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if actor.kind == "operator" || actor.kind == "delegating_agent" {
        Ok(())
    } else {
        Err(error_response(
            StatusCode::FORBIDDEN,
            "Bootstrap Task Creation requires an Operator or Delegating Agent actor",
        ))
    }
}

fn require_child_task_actor(
    actor: &spool_db::Actor,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if actor.kind == "operator" || actor.kind == "delegating_agent" || actor.kind == "worker_agent"
    {
        Ok(())
    } else {
        Err(error_response(
            StatusCode::FORBIDDEN,
            "Child Task creation requires an Operator, Delegating Agent, or Worker Agent actor",
        ))
    }
}

fn require_workpad_actor(actor: &spool_db::Actor) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if actor.kind == "operator" || actor.kind == "delegating_agent" || actor.kind == "worker_agent"
    {
        Ok(())
    } else {
        Err(error_response(
            StatusCode::FORBIDDEN,
            "Workpad Note updates require an Operator, Delegating Agent, or Worker Agent actor",
        ))
    }
}

fn require_task_link_actor(
    actor: &spool_db::Actor,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if actor.kind == "operator" || actor.kind == "delegating_agent" || actor.kind == "worker_agent"
    {
        Ok(())
    } else {
        Err(error_response(
            StatusCode::FORBIDDEN,
            "Task Link updates require an Operator, Delegating Agent, or Worker Agent actor",
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

fn task_mutation_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    let message = error.to_string();
    if message.contains("Worker Agents cannot create Waivers")
        || message.contains("Waivers require an Operator or Review Agent")
        || message.contains("require a Worker Agent actor")
        || message.contains("Worker Agent cannot")
        || message.contains("Child Task creation requires")
        || message.contains("active Agent Run ID")
        || message.contains("active Claim Lease")
        || message.contains("Review Decisions require")
        || message.contains("Backlog Task refinement requires")
    {
        error_response(StatusCode::FORBIDDEN, message)
    } else if message.contains("not found") {
        error_response(StatusCode::NOT_FOUND, message)
    } else if message.contains("Ready Tasks require")
        || message.contains("invalid Priority")
        || message.contains("invalid Task State")
        || message.contains("invalid requirement status")
        || message.contains("only supports Backlog or Ready")
        || message.contains("only supports Backlog Tasks")
        || message.contains("cannot remove")
        || message.contains("invalid Agent Run outcome")
        || message.contains("State Transition")
        || message.contains("Review Decision")
        || message.contains("already in requested")
        || message.contains("already exists")
        || message.contains("same Task Queue")
        || message.contains("block itself")
        || message.contains("would create a cycle")
        || message.contains("does not block")
        || message.contains("pass gates")
        || message.contains("Ready Tasks require")
        || message.contains("must be positive")
        || message.contains("explicit reason")
        || message.contains("position must")
        || message.contains("must not be blank")
    {
        error_response(StatusCode::BAD_REQUEST, message)
    } else {
        internal_error(error)
    }
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
            .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
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
                            "queue": sample_queue_json("TASK", "Spool")
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
            .oneshot(create_queue_request("TASK", "Spool", "worker", &token))
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
            .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);

        let duplicate = app
            .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
            .await
            .unwrap();
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn blocking_task_relationship_endpoints_add_remove_and_validate_actor() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool.clone());

        assert_eq!(
            app.clone()
                .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            app.clone()
                .oneshot(create_task_request("TASK", "Blocking", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            app.clone()
                .oneshot(create_task_request("TASK", "Blocked", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );

        let forbidden = app
            .clone()
            .oneshot(blocking_task_relationship_request(
                "POST",
                "TASK-2",
                "TASK-1",
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

        let add = app
            .clone()
            .oneshot(blocking_task_relationship_request(
                "POST", "TASK-2", "TASK-1", "operator", &token,
            ))
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);
        let body = axum::body::to_bytes(add.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["blocking_tasks"][0]["identifier"], "TASK-1");

        let duplicate = app
            .clone()
            .oneshot(blocking_task_relationship_request(
                "POST", "TASK-2", "TASK-1", "operator", &token,
            ))
            .await
            .unwrap();
        assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);

        let remove = app
            .clone()
            .oneshot(blocking_task_relationship_request(
                "DELETE", "TASK-2", "TASK-1", "operator", &token,
            ))
            .await
            .unwrap();
        assert_eq!(remove.status(), StatusCode::OK);
        let body = axum::body::to_bytes(remove.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["blocking_tasks"].as_array().unwrap().is_empty());

        let events = spool_db::list_task_audit_events(&pool, "TASK-2")
            .await
            .expect("audit events");
        assert!(events
            .iter()
            .any(|event| event.event_type == "task.blocking_task.added"));
        assert!(events
            .iter()
            .any(|event| event.event_type == "task.blocking_task.removed"));
    }

    #[tokio::test]
    async fn task_endpoints_create_show_status_and_update_workpad() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool);

        let queue = app
            .clone()
            .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
            .await
            .unwrap();
        assert_eq!(queue.status(), StatusCode::CREATED);

        let create = app
            .clone()
            .oneshot(create_task_request(
                "TASK",
                "API Task",
                "delegating_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(create.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["task"]["identifier"], "TASK-1");

        let show = app
            .clone()
            .oneshot(authorized_get("/tasks/TASK-1", &token))
            .await
            .unwrap();
        assert_eq!(show.status(), StatusCode::OK);

        let child = app
            .clone()
            .oneshot(create_child_task_request(
                "TASK-1",
                "Child",
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(child.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(child.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["task"]["identifier"], "TASK-2");

        let start = app
            .clone()
            .oneshot(transition_request(
                "TASK-1",
                "in_progress",
                "operator",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(start.status(), StatusCode::OK);

        let criterion = app
            .clone()
            .oneshot(update_requirement_request(
                "/tasks/TASK-1/acceptance-criteria/1/status",
                "satisfied",
                None,
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(criterion.status(), StatusCode::OK);

        let validation = app
            .clone()
            .oneshot(update_requirement_request(
                "/tasks/TASK-1/validation-items/1/status",
                "passed",
                None,
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(validation.status(), StatusCode::OK);

        let transition = app
            .clone()
            .oneshot(transition_request(
                "TASK-1",
                "integrating",
                "operator",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(transition.status(), StatusCode::OK);
        let body = axum::body::to_bytes(transition.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["task"]["state"], "integrating");

        let update = app
            .clone()
            .oneshot(update_workpad_request(
                "TASK-1",
                "notes",
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(update.status(), StatusCode::OK);
        let body = axum::body::to_bytes(update.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["workpad_note"]["body"], "notes");

        let audit = app
            .clone()
            .oneshot(authorized_get("/tasks/TASK-1/audit-events", &token))
            .await
            .unwrap();
        assert_eq!(audit.status(), StatusCode::OK);
        let body = axum::body::to_bytes(audit.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().len() >= 4);

        let status = app
            .oneshot(authorized_get("/status", &token))
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn task_link_endpoint_upserts_and_returns_updated_task_detail() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool);

        assert_eq!(
            app.clone()
                .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            app.clone()
                .oneshot(create_task_request("TASK", "API Task", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );

        let insert = app
            .clone()
            .oneshot(upsert_task_link_request(
                "TASK-1",
                "work_log",
                "/tmp/spool.log",
                Some("Initial log"),
                false,
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(insert.status(), StatusCode::OK);
        let body = axum::body::to_bytes(insert.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["task"]["identifier"], "TASK-1");
        assert_eq!(json["task_links"].as_array().unwrap().len(), 1);
        assert_eq!(json["task_links"][0]["kind"], "work_log");
        assert_eq!(json["task_links"][0]["target"], "/tmp/spool.log");
        assert_eq!(json["task_links"][0]["label"], "Initial log");

        let update = app
            .clone()
            .oneshot(upsert_task_link_request(
                "TASK-1",
                "work_log",
                "/tmp/spool.log",
                Some("Updated log"),
                false,
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(update.status(), StatusCode::OK);
        let body = axum::body::to_bytes(update.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["task_links"].as_array().unwrap().len(), 1);
        assert_eq!(json["task_links"][0]["label"], "Updated log");

        let primary_branch = app
            .clone()
            .oneshot(upsert_task_link_request(
                "TASK-1",
                "task_branch",
                "spool/TASK-1",
                None,
                true,
                "operator",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(primary_branch.status(), StatusCode::OK);

        let primary_log = app
            .oneshot(upsert_task_link_request(
                "TASK-1",
                "work_log",
                "/tmp/spool.log",
                Some("Updated log"),
                true,
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(primary_log.status(), StatusCode::OK);
        let body = axum::body::to_bytes(primary_log.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let links = json["task_links"].as_array().unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!(
            links
                .iter()
                .filter(|link| link["is_primary"].as_bool().unwrap())
                .count(),
            1
        );
        assert!(links.iter().any(|link| link["kind"] == "work_log"
            && link["target"] == "/tmp/spool.log"
            && link["is_primary"] == true));
    }

    #[tokio::test]
    async fn task_link_endpoint_maps_missing_task_invalid_actor_and_invalid_input_errors() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool);

        assert_eq!(
            app.clone()
                .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            app.clone()
                .oneshot(create_task_request("TASK", "API Task", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );

        let missing = app
            .clone()
            .oneshot(upsert_task_link_request(
                "TASK-999",
                "work_log",
                "/tmp/spool.log",
                None,
                false,
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let invalid_actor_kind = app
            .clone()
            .oneshot(upsert_task_link_request(
                "TASK-1",
                "work_log",
                "/tmp/spool.log",
                None,
                false,
                "review_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(invalid_actor_kind.status(), StatusCode::FORBIDDEN);

        let blank_actor_id = app
            .clone()
            .oneshot(upsert_task_link_request_with_actor_id(
                "TASK-1",
                "work_log",
                "/tmp/spool.log",
                "",
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(blank_actor_id.status(), StatusCode::BAD_REQUEST);

        let blank_kind = app
            .clone()
            .oneshot(upsert_task_link_request(
                "TASK-1",
                "",
                "/tmp/spool.log",
                None,
                false,
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(blank_kind.status(), StatusCode::BAD_REQUEST);

        let blank_target = app
            .oneshot(upsert_task_link_request(
                "TASK-1",
                "work_log",
                "",
                None,
                false,
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(blank_target.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delegation_endpoints_cover_create_and_refine_happy_paths_without_pi() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool);

        let queue = app
            .clone()
            .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
            .await
            .unwrap();
        assert_eq!(queue.status(), StatusCode::CREATED);

        let create = app
            .clone()
            .oneshot(create_delegated_root_task_request(
                "TASK",
                "Delegated",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(create.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["task"]["identifier"], "TASK-1");
        assert_eq!(json["task"]["state"], "ready");
        assert_eq!(json["acceptance_criteria"].as_array().unwrap().len(), 1);

        let invalid = app
            .clone()
            .oneshot(create_delegated_root_task_request("TASK", "", &token))
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

        let unsupported_post = app
            .clone()
            .oneshot(create_delegated_root_task_request_at(
                "/tasks/not-delegated-root",
                "TASK",
                "Wrong path",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(unsupported_post.status(), StatusCode::METHOD_NOT_ALLOWED);

        let backlog = app
            .clone()
            .oneshot(create_backlog_task_request("TASK", "Refine me", &token))
            .await
            .unwrap();
        assert_eq!(backlog.status(), StatusCode::CREATED);

        let refine = app
            .clone()
            .oneshot(refine_backlog_task_request("TASK-2", &token))
            .await
            .unwrap();
        assert_eq!(refine.status(), StatusCode::OK);
        let body = axum::body::to_bytes(refine.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["task"]["identifier"], "TASK-2");
        assert_eq!(json["task"]["title"], "Refined Backlog Task");
        assert_eq!(json["task"]["state"], "ready");
        assert_eq!(
            json["acceptance_criteria"][0]["description"],
            "Refined outcome is clear"
        );
        assert_eq!(
            json["validation_items"][0]["description"],
            "Refinement deterministic test passes"
        );
        assert_eq!(
            json["conflict_hints"][0]["target"],
            "docs/DELEGATION_SESSION.md"
        );
    }

    #[tokio::test]
    async fn review_decision_endpoint_records_approve_decision() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool);

        assert_eq!(
            app.clone()
                .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            app.clone()
                .oneshot(create_task_request(
                    "TASK",
                    "Review",
                    "delegating_agent",
                    &token
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            app.clone()
                .oneshot(transition_request(
                    "TASK-1",
                    "in_progress",
                    "operator",
                    &token
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.clone()
                .oneshot(update_requirement_request(
                    "/tasks/TASK-1/acceptance-criteria/1/status",
                    "satisfied",
                    None,
                    "operator",
                    &token,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.clone()
                .oneshot(update_requirement_request(
                    "/tasks/TASK-1/validation-items/1/status",
                    "passed",
                    None,
                    "operator",
                    &token,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.clone()
                .oneshot(transition_request(
                    "TASK-1",
                    "human_review",
                    "operator",
                    &token
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        let response = app
            .clone()
            .oneshot(review_decision_request(
                "TASK-1",
                "approve",
                None,
                "review_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["task"]["state"], "integrating");

        let audit = app
            .oneshot(authorized_get("/tasks/TASK-1/audit-events", &token))
            .await
            .unwrap();
        let body = axum::body::to_bytes(audit.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().iter().any(|event| {
            event["event_type"] == "task.review_decision_recorded"
                && event["actor_kind"] == "review_agent"
        }));
    }

    #[tokio::test]
    async fn task_endpoints_require_auth_and_valid_actor() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool);
        let queue = app
            .clone()
            .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
            .await
            .unwrap();
        assert_eq!(queue.status(), StatusCode::CREATED);

        let unauthenticated = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tasks/bootstrap")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "actor": { "kind": "operator", "id": "tester", "display_name": "tester" },
                            "task": sample_task_json("TASK", "No Auth")
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let forbidden = app
            .oneshot(create_task_request(
                "TASK",
                "Wrong Actor",
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn agent_run_endpoints_claim_heartbeat_and_finish() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool);
        assert_eq!(
            app.clone()
                .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            app.clone()
                .oneshot(create_task_request("TASK", "Run", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );

        let claim = app
            .clone()
            .oneshot(claim_next_request("TASK", "worker_agent", &token))
            .await
            .unwrap();
        assert_eq!(claim.status(), StatusCode::OK);
        let body = axum::body::to_bytes(claim.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let run_id = json["run"]["id"].as_str().unwrap();
        assert_eq!(json["task"]["task"]["state"], "in_progress");

        let show = app
            .clone()
            .oneshot(authorized_get(&format!("/agent-runs/{run_id}"), &token))
            .await
            .unwrap();
        assert_eq!(show.status(), StatusCode::OK);
        let body = axum::body::to_bytes(show.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["run"]["id"], run_id);
        assert_eq!(json["task"]["task"]["identifier"], "TASK-1");

        let heartbeat = app
            .clone()
            .oneshot(heartbeat_request(run_id, "worker_agent", &token))
            .await
            .unwrap();
        assert_eq!(heartbeat.status(), StatusCode::OK);

        let finish = app
            .clone()
            .oneshot(finish_request(run_id, "completed", "worker_agent", &token))
            .await
            .unwrap();
        assert_eq!(finish.status(), StatusCode::OK);

        let reclaim = app
            .oneshot(claim_next_request("TASK", "worker_agent", &token))
            .await
            .unwrap();
        assert_eq!(reclaim.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn claim_next_requires_worker_agent() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool);
        assert_eq!(
            app.clone()
                .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        let response = app
            .oneshot(claim_next_request("TASK", "operator", &token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn worker_agent_waiver_returns_forbidden() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool);
        let queue = app
            .clone()
            .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
            .await
            .unwrap();
        assert_eq!(queue.status(), StatusCode::CREATED);
        let create = app
            .clone()
            .oneshot(create_task_request("TASK", "API Task", "operator", &token))
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);

        let response = app
            .oneshot(update_requirement_request(
                "/tasks/TASK-1/acceptance-criteria/1/status",
                "waived",
                Some("not needed"),
                "worker_agent",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn invalid_ready_task_returns_bad_request() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool);
        let queue = app
            .clone()
            .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
            .await
            .unwrap();
        assert_eq!(queue.status(), StatusCode::CREATED);

        let request = serde_json::json!({
            "actor": { "kind": "operator", "id": "tester", "display_name": "tester" },
            "task": {
                "queue_key": "TASK",
                "title": "Invalid",
                "brief": "Missing requirements",
                "priority": "normal",
                "state": "ready",
                "review_required": false,
                "acceptance_criteria": [],
                "validation_items": [],
                "tags": []
            }
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tasks/bootstrap")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn task_context_bundle_includes_workflow_context_without_raw_launcher_payloads() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool.clone());
        assert_eq!(
            app.clone()
                .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            app.clone()
                .oneshot(create_task_request("TASK", "Bundle", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            app.clone()
                .oneshot(update_workpad_request(
                    "TASK-1",
                    "run-start notes",
                    "worker_agent",
                    &token,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let mut blocked_task = sample_task_json("TASK", "Blocked by bundle task");
        blocked_task["blocking_task_identifiers"] = serde_json::json!(["TASK-1"]);
        let blocked_request = serde_json::json!({
            "actor": { "kind": "operator", "id": "tester", "display_name": "tester" },
            "task": blocked_task
        });
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/tasks/bootstrap")
                        .header("content-type", "application/json")
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::from(blocked_request.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        let operator = spool_db::Actor::operator("tester");
        spool_db::upsert_task_link(
            &pool,
            "TASK-1",
            &spool_db::UpsertTaskLink {
                kind: "local_worktree".to_string(),
                target: "/worktrees/TASK-1".to_string(),
                label: Some("Local Worktree".to_string()),
                is_primary: true,
            },
            &operator,
        )
        .await
        .unwrap();
        spool_db::upsert_task_link(
            &pool,
            "TASK-1",
            &spool_db::UpsertTaskLink {
                kind: "task_branch".to_string(),
                target: "spool/TASK-1".to_string(),
                label: None,
                is_primary: true,
            },
            &operator,
        )
        .await
        .unwrap();

        let claim = app
            .clone()
            .oneshot(claim_next_request("TASK", "worker_agent", &token))
            .await
            .unwrap();
        assert_eq!(claim.status(), StatusCode::OK);
        let body = axum::body::to_bytes(claim.into_body(), usize::MAX)
            .await
            .unwrap();
        let claim_json: Value = serde_json::from_slice(&body).unwrap();
        let run_id = claim_json["run"]["id"].as_str().unwrap();
        spool_db::upsert_launcher_session_data(
            &pool,
            run_id,
            &spool_db::UpsertLauncherSessionData {
                launcher_kind: "pi".to_string(),
                session_id: Some("session-123".to_string()),
                model: Some("test-model".to_string()),
                provider: Some("test-provider".to_string()),
                started_at: None,
                finished_at: None,
                final_status: Some("running".to_string()),
                transcript_path: Some("/local/private/run.jsonl".to_string()),
                raw_json: Some(r#"{"private":"launcher details"}"#.to_string()),
            },
            &operator,
        )
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO agent_run_metrics (
                agent_run_id, derivation_version, duration_ms, launcher_kind, final_status,
                total_tokens, tool_call_count, tool_error_count,
                repeated_failed_tool_attempt_count, repeated_read_count,
                repeated_spool_context_fetch_count, max_context_tokens, efficiency_hints_json
            ) VALUES (?, 1, 1200, 'pi', 'running', 42, 7, 0, 0, 1, 0, 1000, '["repeated file reads"]')
            ON CONFLICT(agent_run_id) DO UPDATE SET
                derivation_version = excluded.derivation_version,
                duration_ms = excluded.duration_ms,
                final_status = excluded.final_status,
                total_tokens = excluded.total_tokens,
                tool_call_count = excluded.tool_call_count,
                tool_error_count = excluded.tool_error_count,
                repeated_failed_tool_attempt_count = excluded.repeated_failed_tool_attempt_count,
                repeated_read_count = excluded.repeated_read_count,
                repeated_spool_context_fetch_count = excluded.repeated_spool_context_fetch_count,
                max_context_tokens = excluded.max_context_tokens,
                efficiency_hints_json = excluded.efficiency_hints_json
            "#,
        )
        .bind(run_id)
        .execute(&pool)
        .await
        .unwrap();
        spool_db::record_integration_outcome(
            &pool,
            &spool_db::RecordIntegrationOutcomeInput {
                task_identifier: "TASK-1".to_string(),
                agent_run_id: Some(run_id.to_string()),
                outcome_kind: "operational_failure".to_string(),
                reason_code: "unknown_operational_failure".to_string(),
                final_commit: None,
                pre_merge_head: Some("abc123".to_string()),
                message: Some("conflict while merging".to_string()),
                retryable: true,
                retry_attempt: Some(1),
                retry_delay_seconds: Some(60),
            },
            &operator,
        )
        .await
        .unwrap();

        let response = app
            .oneshot(authorized_get("/tasks/TASK-1/context-bundle", &token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json["task"]["task"]["brief"],
            "Implement the requested API behavior."
        );
        assert_eq!(json["task"]["workpad_note"]["body"], "run-start notes");
        assert_eq!(
            json["task"]["conflict_hints"][0]["target"],
            "crates/spool-server/src/lib.rs"
        );
        assert_eq!(
            json["advisory_hints"]["task_conflict_hints"][0]["target"],
            "crates/spool-server/src/lib.rs"
        );
        assert_eq!(
            json["advisory_hints"]["likely_files_or_paths"][0],
            "crates/spool-server/src/lib.rs"
        );
        assert!(json["advisory_hints"]["note"]
            .as_str()
            .unwrap()
            .contains("advisory"));
        assert!(json["advisory_hints"]["note"]
            .as_str()
            .unwrap()
            .contains("do not block claims"));
        assert_eq!(
            json["local_workflow"]["local_worktree"],
            "/worktrees/TASK-1"
        );
        assert_eq!(json["local_workflow"]["task_branch"], "spool/TASK-1");
        assert_eq!(json["queue"]["key"], "TASK");
        assert_eq!(
            json["task"]["acceptance_criteria"][0]["description"],
            "It works"
        );
        assert_eq!(
            json["task"]["validation_items"][0]["description"],
            "cargo test passes"
        );
        assert_eq!(json["task"]["task_links"].as_array().unwrap().len(), 2);
        assert_eq!(json["task"]["blocked_tasks"][0]["identifier"], "TASK-2");
        assert_eq!(json["agent_runs"][0]["id"], run_id);
        assert_eq!(json["agent_runs"][0]["is_active"], true);
        assert_eq!(json["agent_runs"][0]["session_id"], "session-123");
        assert_eq!(json["agent_runs"][0]["model"], "test-model");
        assert_eq!(json["agent_runs"][0]["provider"], "test-provider");
        assert_eq!(json["agent_runs"][0]["final_status"], "running");
        assert_eq!(json["agent_runs"][0]["duration_ms"], 1200);
        assert_eq!(json["agent_runs"][0]["tool_call_count"], 7);
        assert_eq!(json["agent_runs"][0]["repeated_read_count"], 1);
        assert_eq!(
            json["agent_runs"][0]["repeated_spool_context_fetch_count"],
            0
        );
        assert_eq!(json["agent_runs"][0]["total_tokens"], 42);
        assert_eq!(json["agent_runs"][0]["max_context_tokens"], 1000);
        assert_eq!(
            json["agent_runs"][0]["efficiency_hints_json"],
            "[\"repeated file reads\"]"
        );
        assert_eq!(
            json["latest_integration_outcome"]["outcome_kind"],
            "operational_failure"
        );
        assert!(json.to_string().find("raw_json").is_none());
        assert!(json.to_string().find("transcript").is_none());
    }

    #[tokio::test]
    async fn task_context_bundle_allows_absent_optional_fields() {
        let (_temp, pool, token) = migrated_pool().await;
        let app = router("test-version", pool);
        assert_eq!(
            app.clone()
                .oneshot(create_queue_request("TASK", "Spool", "operator", &token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        let mut task = sample_task_json("TASK", "Bundle");
        task.as_object_mut().unwrap().remove("conflict_hints");
        let request = serde_json::json!({
            "actor": {
                "kind": "operator",
                "id": "tester",
                "display_name": "tester"
            },
            "task": task
        });
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/tasks/bootstrap")
                        .header("content-type", "application/json")
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::from(request.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );

        let response = app
            .oneshot(authorized_get("/tasks/TASK-1/context-bundle", &token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["task"]["workpad_note"], Value::Null);
        assert_eq!(
            json["advisory_hints"]["task_conflict_hints"],
            serde_json::json!([])
        );
        assert_eq!(
            json["advisory_hints"]["likely_files_or_paths"],
            serde_json::json!([])
        );
        assert_eq!(json["local_workflow"]["local_worktree"], Value::Null);
        assert_eq!(json["agent_runs"].as_array().unwrap().len(), 0);
        assert_eq!(json["latest_failure"], Value::Null);
        assert_eq!(json["latest_integration_outcome"], Value::Null);
    }

    #[tokio::test]
    async fn missing_task_returns_not_found() {
        let (_temp, pool, token) = migrated_pool().await;
        let response = router("test-version", pool)
            .oneshot(authorized_get("/tasks/MISSING-1", &token))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
        let db_path = temp.path().join("spool.db");
        let pool = spool_db::connect(&db_path).await.expect("connect");
        spool_db::run_migrations(&pool).await.expect("migrate");
        let token = spool_db::ensure_local_api_token(&pool)
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

    fn claim_next_request(queue_key: &str, actor_kind: &str, token: &str) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": actor_kind,
                "id": "worker",
                "display_name": "worker"
            },
            "worker_id": "worker",
            "launcher_kind": "fake",
            "lease_seconds": 90
        });

        Request::builder()
            .method("POST")
            .uri(format!("/queues/{queue_key}/claim-next"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn heartbeat_request(run_id: &str, actor_kind: &str, token: &str) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": actor_kind,
                "id": "worker",
                "display_name": "worker"
            },
            "lease_seconds": 90
        });

        Request::builder()
            .method("POST")
            .uri(format!("/agent-runs/{run_id}/heartbeat"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn finish_request(run_id: &str, outcome: &str, actor_kind: &str, token: &str) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": actor_kind,
                "id": "worker",
                "display_name": "worker"
            },
            "outcome": outcome,
            "failure_reason": null,
            "retry_hold_seconds": null
        });

        Request::builder()
            .method("POST")
            .uri(format!("/agent-runs/{run_id}/finish"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn create_child_task_request(
        parent_identifier: &str,
        title: &str,
        actor_kind: &str,
        token: &str,
    ) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": actor_kind,
                "id": "tester",
                "display_name": "tester"
            },
            "task": {
                "title": title,
                "brief": "Child work",
                "priority": "normal",
                "state": "ready",
                "review_required": false,
                "acceptance_criteria": ["It works"],
                "validation_items": ["cargo test passes"],
                "tags": ["child"],
                "blocks_parent": false
            }
        });

        Request::builder()
            .method("POST")
            .uri(format!("/tasks/{parent_identifier}/child-tasks"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn blocking_task_relationship_request(
        method: &str,
        identifier: &str,
        blocking_identifier: &str,
        actor_kind: &str,
        token: &str,
    ) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": actor_kind,
                "id": "tester",
                "display_name": "tester"
            }
        });

        Request::builder()
            .method(method)
            .uri(format!(
                "/tasks/{identifier}/blocking-tasks/{blocking_identifier}"
            ))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn create_task_request(
        queue_key: &str,
        title: &str,
        actor_kind: &str,
        token: &str,
    ) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": actor_kind,
                "id": "tester",
                "display_name": "tester"
            },
            "task": sample_task_json(queue_key, title)
        });

        Request::builder()
            .method("POST")
            .uri("/tasks/bootstrap")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn create_backlog_task_request(queue_key: &str, title: &str, token: &str) -> Request<Body> {
        let mut task = sample_task_json(queue_key, title);
        task["state"] = serde_json::json!("backlog");
        task["acceptance_criteria"] = serde_json::json!([]);
        task["validation_items"] = serde_json::json!([]);
        let request = serde_json::json!({
            "actor": {
                "kind": "operator",
                "id": "tester",
                "display_name": "tester"
            },
            "task": task
        });

        Request::builder()
            .method("POST")
            .uri("/tasks/bootstrap")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn create_delegated_root_task_request(
        queue_key: &str,
        title: &str,
        token: &str,
    ) -> Request<Body> {
        create_delegated_root_task_request_at("/tasks/delegated-root", queue_key, title, token)
    }

    fn create_delegated_root_task_request_at(
        uri: &str,
        queue_key: &str,
        title: &str,
        token: &str,
    ) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": "delegating_agent",
                "id": "delegator",
                "display_name": "delegator"
            },
            "draft": {
                "queue_key": queue_key,
                "title": title,
                "brief": "Delegated Task Brief",
                "priority": "normal",
                "initial_state": "ready",
                "review_required": false,
                "acceptance_criteria": ["Outcome is clear"],
                "validation_items": ["Deterministic check passes"],
                "tags": ["delegation"],
                "conflict_hints": ["crates/spool-cli"],
                "blocking_task_identifiers": []
            }
        });

        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn refine_backlog_task_request(identifier: &str, token: &str) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": "delegating_agent",
                "id": "delegator",
                "display_name": "delegator"
            },
            "refinement": {
                "title": "Refined Backlog Task",
                "brief": "# Task Brief\n\nReady for a Worker Agent.",
                "priority": "normal",
                "target_state": "ready",
                "review_required": false,
                "acceptance_criteria": ["Refined outcome is clear"],
                "validation_items": ["Refinement deterministic test passes"],
                "tags": ["delegation"],
                "conflict_hints": ["docs/DELEGATION_SESSION.md"],
                "blocking_task_identifiers": []
            }
        });

        Request::builder()
            .method("POST")
            .uri(format!("/tasks/{identifier}/refine"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn transition_request(
        identifier: &str,
        to_state: &str,
        actor_kind: &str,
        token: &str,
    ) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": actor_kind,
                "id": "worker",
                "display_name": "worker"
            },
            "to_state": to_state,
            "agent_run_id": null
        });

        Request::builder()
            .method("POST")
            .uri(format!("/tasks/{identifier}/transition"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn review_decision_request(
        identifier: &str,
        decision: &str,
        feedback: Option<&str>,
        actor_kind: &str,
        token: &str,
    ) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": actor_kind,
                "id": "reviewer",
                "display_name": "reviewer"
            },
            "decision": decision,
            "feedback": feedback
        });

        Request::builder()
            .method("POST")
            .uri(format!("/tasks/{identifier}/review-decision"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn update_requirement_request(
        uri: &str,
        status: &str,
        waiver_reason: Option<&str>,
        actor_kind: &str,
        token: &str,
    ) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": actor_kind,
                "id": "tester",
                "display_name": "tester"
            },
            "status": status,
            "waiver_reason": waiver_reason
        });

        Request::builder()
            .method("PUT")
            .uri(uri)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn update_workpad_request(
        identifier: &str,
        body: &str,
        actor_kind: &str,
        token: &str,
    ) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": actor_kind,
                "id": "tester",
                "display_name": "tester"
            },
            "body": body
        });

        Request::builder()
            .method("PUT")
            .uri(format!("/tasks/{identifier}/workpad"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn upsert_task_link_request(
        identifier: &str,
        kind: &str,
        target: &str,
        label: Option<&str>,
        is_primary: bool,
        actor_kind: &str,
        token: &str,
    ) -> Request<Body> {
        upsert_task_link_request_with_actor(
            identifier, kind, target, label, is_primary, "tester", actor_kind, token,
        )
    }

    fn upsert_task_link_request_with_actor_id(
        identifier: &str,
        kind: &str,
        target: &str,
        actor_id: &str,
        actor_kind: &str,
        token: &str,
    ) -> Request<Body> {
        upsert_task_link_request_with_actor(
            identifier, kind, target, None, false, actor_id, actor_kind, token,
        )
    }

    fn upsert_task_link_request_with_actor(
        identifier: &str,
        kind: &str,
        target: &str,
        label: Option<&str>,
        is_primary: bool,
        actor_id: &str,
        actor_kind: &str,
        token: &str,
    ) -> Request<Body> {
        let request = serde_json::json!({
            "actor": {
                "kind": actor_kind,
                "id": actor_id,
                "display_name": "tester"
            },
            "link": {
                "kind": kind,
                "target": target,
                "label": label,
                "is_primary": is_primary
            }
        });

        Request::builder()
            .method("POST")
            .uri(format!("/tasks/{identifier}/links"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(request.to_string()))
            .unwrap()
    }

    fn sample_task_json(queue_key: &str, title: &str) -> Value {
        serde_json::json!({
            "queue_key": queue_key,
            "title": title,
            "brief": "Implement the requested API behavior.",
            "priority": "normal",
            "state": "ready",
            "review_required": false,
            "acceptance_criteria": ["It works"],
            "validation_items": ["cargo test passes"],
            "tags": ["api"],
            "conflict_hints": ["crates/spool-server/src/lib.rs"]
        })
    }

    fn sample_queue_json(key: &str, name: &str) -> Value {
        serde_json::json!({
            "key": key,
            "name": name,
            "managed_source_repository": "/repo/spool",
            "main_branch": "main",
            "worktree_root": "/worktrees",
            "branch_template": "spool/{task_identifier}",
            "done_worktree_retention": false
        })
    }
}

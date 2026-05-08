use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct VersionResponse {
    pub version: String,
}

pub fn router(app_version: impl Into<String>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/version", get(version))
        .with_state(ServerInfo {
            version: app_version.into(),
        })
}

pub async fn serve(addr: SocketAddr, version: impl Into<String>) -> Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind Tasker Service to {addr}"))?;
    axum::serve(listener, router(version))
        .await
        .context("Tasker Service failed")
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn version(State(info): State<ServerInfo>) -> Json<VersionResponse> {
    Json(VersionResponse {
        version: info.version,
    })
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
        let response = router("test-version")
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
        let response = router("0.1.0-test")
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
}

#![allow(unused_imports)]

use crate::*;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    FromRow, SqlitePool,
};
use std::{fs, future::Future, path::Path, time::Duration};
use tokio::time::sleep;
use uuid::Uuid;

pub async fn ensure_local_api_token(pool: &SqlitePool) -> Result<String> {
    with_sqlite_write_retry(|| async {
        if let Some(token) = get_api_token(pool, LOCAL_TOKEN_NAME).await? {
            return Ok(token);
        }

        let token = format!("spool_{}", Uuid::new_v4().simple());
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

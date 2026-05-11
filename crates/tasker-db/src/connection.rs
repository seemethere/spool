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

pub(crate) async fn with_sqlite_write_retry<T, F, Fut>(mut operation: F) -> Result<T>
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

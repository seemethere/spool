use std::path::Path;

use anyhow::{Context, Result};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};

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
}

# Use explicit-SQL Rust service crates

Spool v1 will use axum for HTTP, sqlx with SQLite for persistence and migrations, clap for CLI commands, tokio for async process/server work, serde for API types, tracing for logs, uuid for internal IDs, and time for timestamps. We will avoid an ORM so claim, lease, and transition transactions remain explicit and easy to audit.

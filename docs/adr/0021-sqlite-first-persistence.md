# Use SQLite as the v1 persistence engine

Tasker v1 will use SQLite for persistence, with a repository layer that does not intentionally block a future Postgres implementation. This fits the minimal local backend goal and the current Rust + SQLite direction, while avoiding the complexity of maintaining multi-database compatibility before the core Symphony-compatible loop is working.

SQLite migrations are applied only through explicit operator paths such as `tasker init` and `tasker db migrate`. Normal commands open the Task Backend in migration check-only mode: they verify that all applied migrations are present with matching checksums and that no pending migrations are required, but they do not mutate `_sqlx_migrations` or upgrade schema. This prevents unintegrated Task Branch code from migrating a shared project database before the migration file exists on the Managed Source Repository Main Branch.

For Local Worktree Delivery queues, `tasker db migrate` refuses by default unless it is run from the configured Managed Source Repository on its Main Branch. Operators may use an explicit override only after verifying recovery or exceptional migration intent. Worker Loop and supervisor startup perform the same compatibility check before claiming work so migration drift fails before creating Agent Runs.

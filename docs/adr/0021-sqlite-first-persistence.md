# Use SQLite as the v1 persistence engine

Tasker v1 will use SQLite for persistence, with a repository layer that does not intentionally block a future Postgres implementation. This fits the minimal local backend goal and the current Rust + SQLite direction, while avoiding the complexity of maintaining multi-database compatibility before the core Symphony-compatible loop is working.

# Use current state records plus an Audit Event log

Spool will store current relational rows for fast reads and claims, and append an Audit Event record for every domain mutation. The Audit Event log is for history, debugging, metrics, and future projections rather than v1 event sourcing, because reconstructing all behavior from events would add unnecessary complexity before the core backend is proven.

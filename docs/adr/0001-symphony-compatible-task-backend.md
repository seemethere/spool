# Build a Symphony-compatible task backend, not a Linear clone

Spool exists to remove Linear as a strict dependency for Symphony-style agent orchestration, so it will implement the minimum task source/sink semantics Symphony agents need rather than a general-purpose issue tracker. This keeps the first version focused on Tasks, Task States, workpad-style notes, and agent-facing read/write operations while deliberately deferring broader Linear-like product features such as rich UI, notifications, cycles, teams, and full project management workflows.

# Spool project summary

Spool is a local-first **Task Backend** for agent-driven development. It owns repository-local **Tasks**, **Task Queues**, **Task States**, **Agent Runs**, **Workpad Notes**, and delivery records so agents and Operators can coordinate work without requiring Linear, GitHub, pull requests, a hosted service, or a web UI.

The first priority is **Dogfooding Readiness**: Spool should be useful enough to build Spool with Spool. The v1 path is CLI-first and local-first: initialize repository-local Spool state, configure a **Task Queue** for **Local Worktree Delivery**, create structured **Tasks**, run a **Worker Loop** with the **Pi Launcher** and **Spool Pi Extension**, inspect structured evidence, and deliver work through **Agent-Gated Integration**.

Implementation uses Rust, SQLite, `axum`, `sqlx`, `clap`, `tokio`, and a TypeScript Spool Pi Extension. Canonical project language and sequencing live in `CONTEXT.md`, `ROADMAP.md`, and the ADRs under `docs/adr/`.

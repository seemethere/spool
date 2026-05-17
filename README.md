# Spool

Spool is a local-first **Task Backend** for agent-driven development. It owns the local **Tasks**, **Task Queues**, **Task States**, **Agent Runs**, Workpad Notes, and delivery records that agents and operators use to coordinate work without making Linear, GitHub, pull requests, a hosted service, or a web UI required infrastructure.

Spool is early v1 software focused on becoming useful enough to build Spool with Spool. The current direction is CLI-first and local-first:

- **Spool Service** and **Spool API** for structured task and run state.
- **Task Queues** with fixed v1 **Task State** lifecycle.
- **Tasks** with Acceptance Criteria, Validation Items, Workpad Notes, Task Links, and Audit Events.
- **Agent Runs** with Claim Leases and launcher-owned execution.
- **Local Worktree Delivery** for repository-local task branches and integration.
- **Pi Launcher** using `pi --mode rpc` plus a minimal **Spool Pi Extension**.

Spool is not a Linear clone, a generic project management system, a GitHub-dependent workflow, or a mature hosted product.

## Repository layout

- `crates/spool-cli` — CLI entry point and operator commands.
- `crates/spool-server` — HTTP Spool Service.
- `crates/spool-db` — SQLite persistence and migrations.
- `crates/spool-config` — local configuration loading.
- `crates/spool-runner` — runner-side workflow pieces such as worker and delivery orchestration.
- `crates/spool-symphony` — Symphony-specific integration boundary outside the core Spool API.
- `extensions/spool-pi` — TypeScript Spool Pi Extension that talks to the HTTP API.

## Build and validate

Spool is a Rust workspace. From the repository root:

```bash
cargo fmt --all -- --check
cargo test --workspace
```

To inspect the CLI while developing from source:

```bash
cargo run -p spool-cli --bin spool -- --help
```

## First run

For the local-first happy path in an existing repository, start with [`docs/FIRST_RUN_QUICKSTART.md`](docs/FIRST_RUN_QUICKSTART.md). It covers initializing repository-local Spool state, creating a **Task Queue** for **Local Worktree Delivery**, starting the **Spool Service**, creating a **Root Task** through a **Delegation Session**, launching a **Worker Loop**, and inspecting the result.

Before enabling **Local Worktree Delivery**, read the quickstart warning about choosing a **Managed Source Repository** that Spool/Symphony tooling may mutate by creating Local Worktrees, Task Branches, local squash merges, and cleanup operations.

## Project context

The canonical language and scope live in:

- [`CONTEXT.md`](CONTEXT.md) — Spool domain language and relationships.
- [`ROADMAP.md`](ROADMAP.md) — current implementation sequence, with Dogfooding Readiness first.
- [`docs/adr/`](docs/adr/) — architecture decisions for workflow, persistence, delivery, launcher behavior, and related boundaries.

Use those documents when changing Spool behavior or public terminology.

## Contributing and security

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for local development expectations and [`SECURITY.md`](SECURITY.md) for vulnerability reporting guidance.

## License

Spool is licensed under the MIT License. See [`LICENSE`](LICENSE).

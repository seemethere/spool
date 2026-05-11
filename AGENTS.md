# Tasker Agent Instructions

## Always read first

- `CONTEXT.md` — canonical domain language and relationships.
- `ROADMAP.md` — current implementation sequence, with Dogfooding Readiness first.
- Relevant `docs/adr/*.md` files before changing architecture, workflow, persistence, delivery, or launcher behavior.

Use the terms from `CONTEXT.md` exactly. Avoid Linear/GitHub/PR-shaped language unless discussing future optional integrations.

## Product direction

Tasker is a local-first task backend for speeding up agent-driven development. The first priority is Dogfooding Readiness: Tasker should become useful enough to build Tasker with Tasker as quickly as possible.

Keep v1 focused on:

- Tasker Service + Tasker API
- Task Queues
- Tasks, Task States, requirements, Workpad Notes
- Agent Runs and Claim Leases
- Local Worktree Delivery
- Pi Launcher using `pi --mode rpc`
- Minimal Tasker Pi Extension
- CLI-first observability

Do not expand v1 into:

- A Linear clone or generic project management system
- A GitHub/PR-dependent workflow
- A web UI/dashboard
- Multi-tenant permissions/ACLs
- Import/sync from external trackers
- Custom workflows beyond the fixed v1 Task State lifecycle

## Implementation stack

Core Tasker implementation:

- Rust
- `axum` for HTTP
- `sqlx` + SQLite for persistence/migrations
- `clap` for CLI
- `tokio` for async process/server work
- `serde` for API types
- `tracing` for logs
- `uuid` for internal IDs
- `time` for timestamps

Pi integration:

- TypeScript Tasker Pi Extension
- Communicate with Tasker through the HTTP API
- Do not share in-process code between Rust Tasker and the extension

## Dogfooding Readiness path

Prioritize these milestones in order:

1. Rust skeleton, config/init/migrations, health/version.
2. Queues, Tasks, requirements, Workpad Notes, Audit Events, bootstrap creation, show/status CLI.
3. Claim/lease/run lifecycle and fake launcher worker loop.
4. Local Worktree Delivery setup, Pi Launcher RPC, minimal Tasker Pi Extension.
5. Run transcripts/session data, status/run show, manual dogfood merge or first Integrating implementation, deterministic tests.

Temporary dogfooding escape hatches are allowed only when clearly marked:

- `tasker task create --bootstrap --queue <key> --file task.md`
- Manual Dogfood Merge before automatic Integrating is implemented

These do not replace the target model.

## Agent efficiency rules

Efficiency is a first-class dogfooding concern. Optimize for fewer tokens, fewer tool calls, and less repeated context discovery while preserving correctness.

Before broad exploration:

- Read `CONTEXT.md`, `ROADMAP.md`, and only the ADRs/docs relevant to the Task area.
- Use the Task Brief, Acceptance Criteria, Validation Items, Task Links, Workpad Note, and Task Conflict Hints to make a short context plan before reading many files.
- Prefer targeted `rg`/`find` queries and narrow `read` ranges over opening large files end-to-end.
- Avoid rereading unchanged files. Keep notes in your reasoning about files and symbols already inspected.
- Avoid broad SQL, transcript parsing, or repeated `tasker status`/`task show` loops unless the Task is explicitly about observability or telemetry.

During implementation:

- Start with the smallest plausible change that satisfies the Acceptance Criteria.
- Prefer focused deterministic tests over full-suite runs until the final validation step.
- Do not run expensive commands repeatedly after unrelated edits; batch validation when safe.
- Keep Workpad Notes concise: summary, changed files, validation, risks, and follow-up Task candidates.

For Tasker workflow updates, prefer Tasker Pi Extension tools when available. Use `bin/tasker-local` for operator/debug reads and fallback workflow mutations only when needed.

When investigating efficiency, cite numeric summaries and local artifact paths, not raw prompt bodies, raw transcripts, secrets, or large pasted logs. Token/cache/context metrics are local-only and should come from Tasker telemetry when available.

## Project dogfooding command safety

Project dogfooding commands must use the project Tasker database, not the default user Tasker database. Prefer the repo-local `bin/tasker-local` wrapper for project dogfood CLI reads and operator/debug commands. It runs `cargo run -p tasker-cli --bin tasker` from the Managed Source Repository root with the repository's `.tasker/config.toml`, so the CLI rebuilds automatically when needed. This favors correctness over fastest startup during dogfooding; there is no separate fast path currently.

If the wrapper is unavailable, run Tasker CLI commands from the Managed Source Repository root and pass the project config explicitly:

```bash
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data <tasker-args>
```

Do not run bare `tasker task create`, `tasker status`, `tasker work`, or `tasker supervise` from this repository. Bare commands can read or mutate the wrong Task Backend.

Before any Tasker mutation for project dogfooding, run this preflight and confirm it prints `key: TASKER`:

```bash
bin/tasker-local queue show TASKER
```

Only continue with project dogfooding mutations when the preflight shows the `TASKER` Task Queue from the project database.

## Architectural rules

- Tasker records delivery configuration and outcomes; Delivery Adapters perform filesystem/Git operations outside Tasker.
- Tasker records Agent Runs and Launcher Session Data; Agent Launchers execute agents outside Tasker.
- Local Worktree Delivery uses a Managed Source Repository. Warn operators that Tasker/Symphony may mutate it.
- Use explicit SQL for claim, lease, transition, and delivery transactions.
- Current relational rows are authoritative; Audit Events are append-only history, not v1 event sourcing.
- Structured Tasker fields are authoritative for gates and scheduling; Workpad Note Markdown is narrative/handoff context.

## Testing strategy

Prefer deterministic tests:

- Temp SQLite databases
- Temp Git repositories
- Fake Agent Launchers
- Fake Delivery Adapter outcomes
- Contract tests for the Tasker Pi Extension against a test Tasker server

Keep real pi end-to-end tests opt-in because they require local model credentials and agent availability.

## Documentation discipline

Update documentation in the same change when behavior or domain meaning changes:

- Update `CONTEXT.md` for domain language/relationships.
- Add or update ADRs only for decisions that are hard to reverse, surprising without context, and trade-off driven.
- Update `ROADMAP.md` when milestone sequencing changes.

## Commit style

Use Conventional Commits, for example:

- `feat: add bootstrap task creation`
- `fix: prevent duplicate task claims`
- `docs: update local worktree delivery ADR`
- `test: cover claim lease expiry`
- `chore: configure rust workspace`

Keep commits focused and prefer small checkpoints before large implementation steps.

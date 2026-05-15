# Spool Agent Instructions

## Always read first

- `CONTEXT.md` — canonical domain language and relationships.
- `ROADMAP.md` — current implementation sequence, with Dogfooding Readiness first.
- Relevant `docs/adr/*.md` files before changing architecture, workflow, persistence, delivery, or launcher behavior.

Use the terms from `CONTEXT.md` exactly. Avoid Linear/GitHub/PR-shaped language unless discussing future optional integrations.

## Product direction

Spool is a local-first task backend for speeding up agent-driven development. The first priority is Dogfooding Readiness: Spool should become useful enough to build Spool with Spool as quickly as possible.

Keep v1 focused on:

- Spool Service + Spool API
- Task Queues
- Tasks, Task States, requirements, Workpad Notes
- Agent Runs and Claim Leases
- Local Worktree Delivery
- Pi Launcher using `pi --mode rpc`
- Minimal Spool Pi Extension
- CLI-first observability

Do not expand v1 into:

- A Linear clone or generic project management system
- A GitHub/PR-dependent workflow
- A web UI/dashboard
- Multi-tenant permissions/ACLs
- Import/sync from external trackers
- Custom workflows beyond the fixed v1 Task State lifecycle

## Implementation stack

Core Spool implementation:

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

- TypeScript Spool Pi Extension
- Communicate with Spool through the HTTP API
- Do not share in-process code between Rust Spool and the extension

## Dogfooding Readiness path

Prioritize these milestones in order:

1. Rust skeleton, config/init/migrations, health/version.
2. Queues, Tasks, requirements, Workpad Notes, Audit Events, bootstrap creation, show/status CLI.
3. Claim/lease/run lifecycle and fake launcher worker loop.
4. Local Worktree Delivery setup, Pi Launcher RPC, minimal Spool Pi Extension.
5. Run transcripts/session data, status/run show, manual dogfood merge or first Integrating implementation, deterministic tests.

Temporary dogfooding escape hatches are allowed only when clearly marked:

- `spool task create --bootstrap --queue <key> --file task.md`
- Manual Dogfood Merge before automatic Integrating is implemented

These do not replace the target model.

## Dogfood review and integration default

Spool's own dogfood **Task Queue** defaults to **Agent-Gated Integration**, not **Human Review**. After structured **Acceptance Criteria** are satisfied or waived and **Validation Items** are passed or waived, ordinary dogfood **Tasks** should proceed to **Integrating**.

Use **Human Review** only when the **Task** or **Task Queue** explicitly requires it, such as `review_required: true`, or when a human/**Operator** asks for it. When extra confidence is needed for Spool development, prefer an advisory **Subagent Review Loop** before committing or requesting **Integrating**. Advisory subagents do not replace Spool's domain **Review Agent**, **Review Session**, or **Review Decision**.

## Agent efficiency rules

Efficiency is a first-class dogfooding concern. Optimize for fewer tokens, fewer tool calls, and less repeated context discovery while preserving correctness.

Before broad exploration:

- Read `CONTEXT.md`, `ROADMAP.md`, and only the ADRs/docs relevant to the Task area.
- Use the Task Brief, Acceptance Criteria, Validation Items, Task Links, Workpad Note, and Task Conflict Hints to make a short context plan before reading many files.
- Prefer targeted `rg`/`find` queries and narrow `read` ranges over opening large files end-to-end.
- Avoid rereading unchanged files. Keep notes in your reasoning about files and symbols already inspected.
- Avoid broad SQL, transcript parsing, or repeated `spool status`/`spool task show` loops unless the Task is explicitly about observability or telemetry.

During implementation:

- Start with the smallest plausible change that satisfies the Acceptance Criteria.
- Prefer focused deterministic tests over full-suite runs until the final validation step.
- Do not run expensive commands repeatedly after unrelated edits; batch validation when safe.
- Keep Workpad Notes concise: summary, changed files, validation, risks, and follow-up Task candidates.

For Spool workflow updates, prefer Spool Pi Extension tools when available. Use `bin/spool-local` for operator/debug reads and fallback workflow mutations only when needed.

When investigating efficiency, cite numeric summaries and local artifact paths, not raw prompt bodies, raw transcripts, secrets, or large pasted logs. Token/cache/context metrics are local-only and should come from Spool telemetry when available.

## Project dogfooding command safety

Project dogfooding commands must use the project Spool database, not the default user Spool database. Prefer the repo-local `bin/spool-local` wrapper for project dogfood CLI reads and operator/debug commands. It runs `cargo run -p spool-cli --bin spool` from the Managed Source Repository root with the repository's `.spool/config.toml`, so the CLI rebuilds automatically when needed. This favors correctness over fastest startup during dogfooding; there is no separate fast path currently.

If the wrapper is unavailable, run Spool CLI commands from the Managed Source Repository root and pass the project config explicitly:

```bash
cargo run -p spool-cli -- --config .spool/config.toml --data-dir .spool/data <spool-args>
```

The old Tasker name may appear only in explicitly historical rename-migration context, such as `.tasker/` source data consumed by `scripts/migrate-dogfood-state-to-spool.sh`, historical SQLite migrations, or completed pre-rename dogfood records. Do not introduce new canonical docs, APIs, commands, crates, paths, examples, or queue keys with the old name.

Do not run bare `spool task create`, `spool status`, `spool work`, or `spool supervise` from this repository. Bare commands can read or mutate the wrong Task Backend.

Before any Spool mutation for project dogfooding, run this preflight and confirm it prints `key: SPOOL`:

```bash
bin/spool-local queue show SPOOL
```

Only continue with project dogfooding mutations when the preflight shows the `SPOOL` Task Queue from the project database.

## Architectural rules

- Spool records delivery configuration and outcomes; Delivery Adapters perform filesystem/Git operations outside Spool.
- Spool records Agent Runs and Launcher Session Data; Agent Launchers execute agents outside Spool.
- Local Worktree Delivery uses a Managed Source Repository. Warn operators that Spool/Symphony may mutate it.
- Use explicit SQL for claim, lease, transition, and delivery transactions.
- Current relational rows are authoritative; Audit Events are append-only history, not v1 event sourcing.
- Structured Spool fields are authoritative for gates and scheduling; Workpad Note Markdown is narrative/handoff context.

## Testing strategy

Prefer deterministic tests:

- Temp SQLite databases
- Temp Git repositories
- Fake Agent Launchers
- Fake Delivery Adapter outcomes
- Contract tests for the Spool Pi Extension against a test Spool server

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

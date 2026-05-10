# Tasker Implementation Plan

This plan implements **Dogfooding Readiness** first, then continues toward the full v1 local loop. It follows `CONTEXT.md`, `ROADMAP.md`, and the ADRs in `docs/adr/`.

## Goal

Make Tasker useful enough to build Tasker with Tasker as quickly as possible:

1. Initialize local state and run the **Tasker Service**.
2. Create **Task Queues** and bootstrap **Tasks** with structured requirements.
3. Claim **Tasks**, persist **Agent Runs** and **Claim Leases**, and run a fake worker.
4. Prepare **Local Worktrees**, launch pi through **Pi RPC Sessions**, and expose a minimal **Tasker Pi Extension**.
5. Inspect runs and status, then dogfood with either **Manual Dogfood Merge** or the first **Integrating** implementation.

Do not expand this slice into a web UI, Linear-compatible facade, GitHub-dependent workflow, ACL system, or custom workflow engine.

## Cross-cutting rules

- Use Tasker domain language exactly: **Task Queue**, **Task**, **Task State**, **Workpad Note**, **Agent Run**, **Claim Lease**, **Local Worktree Delivery**.
- Keep the **Tasker API** as the first-class integration contract.
- Every domain mutation must include **Actor** attribution and append an **Audit Event**.
- Current relational rows are authoritative; **Audit Events** are history, not v1 event sourcing.
- Use explicit SQL transactions for queue sequence allocation, claims, leases, transitions, and delivery outcomes.
- Keep filesystem and Git delivery operations in the runner-side **Delivery Adapter**, not in the **Tasker Service**.
- Prefer deterministic tests with temp SQLite databases, temp Git repositories, fake **Agent Launchers**, and fake **Delivery Adapter** outcomes.
- Keep real pi end-to-end tests opt-in.

## Proposed workspace layout

```text
Cargo.toml
crates/
  tasker-core/        # domain enums, transition/gate logic, shared API structs
  tasker-db/          # SQLite migrations, repository methods, explicit transactions
  tasker-server/      # axum router and HTTP handlers
  tasker-worker/      # Worker Loop, Symphony Adapter boundary, launchers, delivery adapters
  tasker-cli/         # clap binary: init, serve, queue, task, work, status, run
extensions/
  tasker-pi/          # TypeScript Tasker Pi Extension
```

The Rust crates can start small, but keep domain logic out of HTTP handlers so the CLI, service, and tests use the same behavior.

## Persistence plan

Add migrations incrementally by milestone.

### Core tables

- `api_tokens`
- `task_queues`
- `tasks`
- `acceptance_criteria`
- `validation_items`
- `workpad_notes`
- `workpad_revisions`
- `task_tags`
- `task_links`
- `task_relationships`
- `agent_runs`
- `delivery_records`
- `integration_outcomes`
- `launcher_session_data`
- `audit_events`

### Important constraints

- Unique **Task Queue Key**.
- Queue-local sequence allocation inside the same transaction as **Task** creation.
- Unique **Task Identifier** generated as `<TASK_QUEUE_KEY>-<sequence>`.
- At most one active **Workpad Note** per **Task**.
- At most one **Primary Handoff Link** per **Task**.
- At most one active **Agent Run** per **Task**; claims should expire stale runs before inserting a new active run.
- State, priority, requirement status, validation status, run outcome, and delivery outcome values should be constrained in Rust and SQLite.

## Milestone 1: Skeleton and local state

### Implement

1. Create the Rust workspace and `tasker` CLI binary.
2. Add config loading:
   - default config: `~/.config/tasker/config.toml`
   - default data dir: `~/.local/share/tasker/`
   - default SQLite path: `~/.local/share/tasker/tasker.db`
   - explicit override flags/env vars for tests.
3. Implement `tasker init`:
   - create config/data directories
   - create or migrate SQLite database
   - create a local bearer token if missing
   - write files with restrictive permissions where appropriate.
4. Add migration runner using `sqlx`.
5. Implement `tasker serve` with `axum`, `tokio`, and `tracing`.
6. Add unauthenticated endpoints:
   - `GET /health`
   - `GET /version`

### Acceptance check

- `tasker init` creates config, data directory, database, and token.
- `tasker serve` binds to `127.0.0.1` by default.
- `/health` and `/version` work without auth.
- Temp-database migration tests pass.

## Milestone 2: Core task data

### Implement

1. Queue API and CLI:
   - `tasker queue create`
   - `tasker queue show`
   - `tasker queue list`
   - API equivalents.
2. Local Worktree Delivery queue config fields:
   - **Managed Source Repository**
   - **Main Branch**
   - **Worktree Root**
   - **Branch Template**
   - **Done Worktree Retention**.
3. Warn the **Operator** during Local Worktree Delivery queue creation that Tasker/Symphony may mutate the **Managed Source Repository**.
4. Core **Task** persistence:
   - title
   - **Task Brief**
   - **Priority**
   - **Task State**
   - **Acceptance Criteria**
   - **Validation Items**
   - tags
   - review requirement.
5. Bootstrap creation:
   - `tasker task create --bootstrap --queue <key> --file task.md`
   - YAML front matter for structured fields
   - Markdown body as the **Task Brief**
   - default state: **Ready** when omitted.
6. Enforce gates:
   - **Ready** requires at least one **Acceptance Criterion** and one **Validation Item** unless using a **Repair Override**.
   - transitions to **Human Review**, **Integrating**, or **Done** require structured completion evidence unless using a **Repair Override**.
7. Workpad API and CLI:
   - read/update active **Workpad Note**
   - save **Workpad Revisions**.
8. Requirement status APIs:
   - set **Criterion Status**
   - set **Validation Status**
   - reject Worker Agent waivers.
9. Status/show commands:
   - `tasker task show <task_identifier>`
   - `tasker status` with queue counts and basic active-run placeholders.
10. Append **Audit Events** for all mutations.

### Acceptance check

- An **Operator** can create a **Task Queue**.
- A bootstrap Markdown file creates a **Task** with a generated **Task Identifier**.
- `tasker task show` displays current structured fields and **Workpad Note**.
- `tasker status` displays counts by **Task Queue** and **Task State**.
- Gate, identifier, audit, and Workpad revision tests pass.

### Dogfooding checkpoint

After this milestone, create real bootstrap **Tasks** for the remaining milestones.

## Milestone 3: Claim and run lifecycle

### Implement

1. Claim eligibility query:
   - state is **Ready**, **In Progress**, **Rework**, or **Integrating**
   - no unresolved **Blocking Tasks**
   - no active **Claim Lease**
   - no active **Retry Hold**
   - queue is below its optional **Queue Concurrency Limit** based on active **Agent Runs**, including active **Integrating** runs
   - ordering: **Priority**, creation time, **Task Identifier**.
2. `claim-next` API:
   - expire stale active **Agent Runs** first
   - atomically create an **Agent Run** and **Claim Lease**
   - move **Ready** to **In Progress** on claim
   - keep **In Progress**, **Rework**, and **Integrating** states unchanged on claim.
3. `heartbeat` API:
   - extend lease expiry
   - record heartbeat timestamp.
4. `finish-run` API:
   - record **Agent Run Outcome**
   - release the **Claim Lease**
   - create **Retry Holds** for failed or expired runs
   - do not directly change **Task State**.
5. Implement `tasker work --once` with a fake **Agent Launcher**:
   - claim one **Task**
   - heartbeat while running
   - append/update **Workpad Note** with fake evidence
   - finish the **Agent Run**.

### Acceptance check

- Concurrent claims cannot claim the same **Task**.
- Expired leases can be reclaimed after expiry and retry hold handling.
- Queue concurrency limits are enforced during claim and count active **Integrating** **Agent Runs** during dogfooding.
- Finishing a run does not silently change **Task State**.
- `tasker work --once --launcher fake` can process one bootstrap **Task** deterministically.

## Milestone 4: Local worktrees and pi

### Implement

1. Runner-side **Local Worktree Delivery** setup:
   - validate **Managed Source Repository**
   - create or reuse the **Task Branch**
   - create the per-Task **Local Worktree** under the **Worktree Root**
   - attach/update **Task Links** for worktree path and branch.
2. Keep Git/filesystem operations in the **Delivery Adapter** and only record delivery facts/outcomes through the **Tasker API**.
3. Implement the **Pi Launcher**:
   - spawn `pi --mode rpc`
   - communicate over JSONL stdin/stdout
   - start one fresh **Pi RPC Session** per **Agent Run**
   - load Worker **Role Prompt** with optional `.tasker/prompts/worker.md` override
   - fail unattended worker runs on unexpected question UI.
4. Add the minimal TypeScript **Tasker Pi Extension**:
   - get **Task**
   - update **Workpad Note**
   - set requirement status
   - create **Child Task**
   - request **State Transition**.
5. Add state transition request API:
   - enforce normal transition graph
   - enforce structured gates
   - allow Worker Agent transition to **Integrating** when gates pass and review policy allows **Agent-Gated Integration**.

### Acceptance check

- Fake launcher can run inside a temp **Local Worktree**.
- Contract tests prove the **Tasker Pi Extension** works against a test **Tasker Service**.
- Opt-in pi smoke test can claim a **Task**, read it through the extension, update the **Workpad Note**, and request a transition.

## Milestone 5: Dogfood hardening

### Implement

1. Persist **Run Transcripts** under the Tasker data directory.
2. Persist **Launcher Session Data**:
   - launcher kind
   - session ID
   - model/provider when available
   - timestamps
   - final status
   - raw launcher-specific JSON/artifacts.
3. Add `tasker run show <run_id>`.
4. Improve `tasker status`:
   - queue counts
   - active **Agent Runs**
   - expiring/expired leases
   - retry holds
   - recent failures.
5. Add deterministic tests for:
   - temp SQLite databases
   - temp Git repositories
   - fake launchers
   - fake delivery outcomes.
6. Choose the fastest dogfooding delivery path:
   - acceptable early path: completed **Local Worktree** plus **Manual Dogfood Merge**
   - preferred path: first **Integrating** path with **Squash Merge**, **Integration Outcome**, **Final Commit**, and cleanup.
7. Implement the first Agent-Gated **Integrating** slice described in `docs/AGENT_GATED_INTEGRATING_PLAN.md`:
   - keep Git/filesystem operations in a runner-side **Delivery Adapter**
   - require an already-**Integrating** Task for the first command-oriented slice
   - record success, no-change, work-change failure, or operational failure outcomes
   - move successful work to **Done**, work-change failures to **Rework**, and leave operational failures in **Integrating** for retry.

### Acceptance check

- A real Tasker development **Task** can be created, claimed, run, inspected, and delivered by **Manual Dogfood Merge** or first **Integrating**.
- The resulting **Agent Run**, **Run Transcript**, **Launcher Session Data**, **Workpad Note**, and status summary are inspectable from the CLI.

## Full v1 follow-up after dogfooding

- Complete Agent-Gated **Integrating** polish beyond the first dogfood slice, including automatic Worker Loop invocation after a Worker Agent transitions to **Integrating** while still holding the **Claim Lease**.
- Add `tasker delegate` and the Pi-backed **Delegation Session**.
- Add `tasker review` and **Review Session** support.
- Expand the **Tasker Pi Extension** with Task Links and richer transition/update tools.
- Add metrics export derived from **Audit Events**, **Agent Runs**, **Launcher Session Data**, and **Integration Outcomes**.
- Add transcript pruning/export commands.
- Revisit worker concurrency beyond the single-worker dogfooding path.

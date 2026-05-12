# Tasker Roadmap

## Dogfooding Readiness

Tasker should become useful enough to build Tasker with Tasker as quickly as possible.

### Milestone 1: Skeleton and local state

- Rust project skeleton
- Config loading
- `tasker init`
- SQLite migrations
- `tasker serve`
- Health/version endpoint

### Milestone 2: Core task data

- Queue create/show/list
- File-backed Task Creation from Markdown + YAML front matter, with the temporary bootstrap command spelling kept as a compatibility path during dogfooding
- Task show/status CLI
- Acceptance criteria and validation items
- Workpad note and revisions
- Audit events

Once this milestone works, create real dogfood tasks for later milestones.

Early dogfooding task chain: simplify the **Worker Agent Contribution Surface** by extracting runner-side execution and delivery behavior into `tasker-runner`, following `docs/RUNNER_EXTRACTION_PLAN.md`. This should be managed as dependency-chained Tasker Tasks after the minimum dogfooding cutover point, not as a prerequisite for cutover.

### Milestone 3: Claim and run lifecycle

- Claim-next
- Lease heartbeat/expiry
- Finish-run
- Retry holds
- Fake Agent Launcher
- `tasker work --once` with fake worker

### Milestone 4: Local worktrees and pi

- Local Worktree Delivery setup
- Pi Launcher using `pi --mode rpc`
- Minimal Tasker Pi Extension tools:
  - get task
  - update workpad
  - set requirement status
  - create child task
  - request transition

### Milestone 5: Dogfood hardening

- Run transcripts and launcher session data
- `tasker run show`
- CLI-first cleanup for dogfood storage artifacts
- Improved `tasker status`
- Manual dogfood merge documentation before automatic Integrating
- First Agent-Gated Integrating slice:
  - runner-side Local Worktree Delivery adapter for already-Integrating Tasks
  - default Squash Merge into Main Branch
  - Integration Outcome recording and Done/Rework/retry state handling
  - cleanup of Local Worktree and Task Branch after success
- Opt-in real pi smoke documentation
- Tests with temp SQLite DBs, temp Git repos, fake launchers, and fake delivery outcomes

## Full v1 after dogfooding

- Complete Agent-Gated Integrating polish beyond the first dogfood slice
- Implement the first Delegation Session contract in `docs/DELEGATION_SESSION.md`, including `tasker delegate`, `tasker delegate --refine <task_identifier>`, deterministic creation/refinement helpers, and the Pi-backed Delegation Interview
- Review sessions
- Metrics export
- Richer transcript export/pruning policies
- Richer Tasker Pi Extension tool set
- Optional concurrency beyond one local worker

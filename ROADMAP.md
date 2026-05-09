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
- Bootstrap task creation from Markdown + YAML front matter
- Task show/status CLI
- Acceptance criteria and validation items
- Workpad note and revisions
- Audit events

Once this milestone works, create real dogfood tasks for later milestones.

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
- Improved `tasker status`
- Manual dogfood merge documentation before automatic Integrating
- Opt-in real pi smoke documentation
- Tests with temp SQLite DBs, temp Git repos, fake launchers, and fake delivery outcomes

## Full v1 after dogfooding

- Agent-Gated Integrating with squash merge
- Review sessions
- Metrics export
- Transcript pruning
- Richer Tasker Pi Extension tool set
- Optional concurrency beyond one local worker

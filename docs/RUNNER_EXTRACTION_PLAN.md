# Spool Runner Extraction Plan

Spool will simplify the **Worker Agent Contribution Surface** by extracting runner-side execution and delivery behavior from `spool-cli` into a new Rust crate named `spool-runner`.

Initial scope:

- Move execution and delivery behavior first: Worker Loop, supervisor orchestration, Agent Launcher handling, Local Worktree setup, Local Worktree Integration, and Managed Source Repository operation locking.
- Put reusable Local Worktree Integration and Managed Source Repository operation-lock mechanics in `spool-runner`; keep manual merge command UX, guidance text, and inspection formatting in `spool-cli`.
- `spool merge retry` should call the shared `spool-runner` integration implementation so there is one delivery path.
- Introduce a small internal Agent Launcher interface with `FakeLauncher` and `PiLauncher` implementations only; do not add dynamic plugin loading during extraction.
- Expose command-shaped request/outcome functions to `spool-cli`, such as `run_worker_once`, `preflight_worker_claim`, `supervise_batch`, `integrate_local_worktree_for_run`, and operation-lock functions/types.
- Keep CLI rendering, operator guidance text, and low-level Git helpers out of the public `spool-runner` API where possible.
- Leave CLI-first observability such as monitor and telemetry in `spool-cli` for now.
- Keep `spool-cli` as a thin command facade over `spool-runner` behavior.
- Move execution/delivery behavior tests with the behavior into `spool-runner`; leave CLI parsing and output smoke tests in `spool-cli`.

Transitional boundary:

- The first extraction keeps direct `spool-db` calls to preserve existing behavior and make the move reviewable.
- This is not the long-term target boundary for the **Symphony Adapter**. The documented target remains interaction through the **Spool API**.
- A later design slice should decide when and how to introduce an HTTP Spool API client boundary for runner-side workflow code.

Out of scope for the first extraction:

- Task State lifecycle changes.
- Spool API semantic changes.
- SQLite schema or migration changes.
- Telemetry and monitor refactors.
- Introducing the HTTP Spool API client boundary.
- CLI output redesign.
- Feature deletion as simplification.

The first extraction should be behavior-preserving apart from moving code and relocating execution/delivery behavior tests.

Suggested Implementation Slice sequence:

1. Add the `spool-runner` crate and move Managed Source Repository operation-lock behavior plus tests.
2. Move Local Worktree Integration and wire `spool merge retry` through `spool-runner`.
3. Move Agent Launcher pieces and Worker Loop behavior.
4. Move supervisor orchestration.
5. Clean up remaining CLI tests into CLI smoke tests and document targeted runner test commands. (Completed by moving remaining Local Worktree Delivery and commit metadata behavior tests into `spool-runner` and adding `crates/spool-runner/README.md` as the canonical targeted runner validation guide.)

Stop between slices if public API shape, behavior, or test failures get messy.

After **Dogfooding Cutover**, model this sequence as one planning/container Root Task with five code-delivery Child Tasks. Each later Child Task should have an explicit Blocking Task relationship on the previous Child Task; parent/child lineage alone is not a dependency in Spool. The Root Task's gates should cover decomposition, dependency wiring, child-task gate quality, and final verification rather than duplicating child code-delivery scope. Each Child Task should deliver independently through its own Local Worktree and Final Commit; the next Child Task starts from Main Branch after the previous one is integrated.

Default to Agent-Gated Integration for mechanical extraction Child Tasks. Require Human Review for the slice that establishes the new public `spool-runner` API/ADR, and for any slice where a Worker Agent discovers behavior changes, public API ambiguity, or validation failures that need interpretation.

Validation standard:

- Each slice runs `cargo fmt --check`.
- Each slice runs targeted tests for touched crates/modules.
- Once `spool-runner` exists, each slice runs `cargo test -p spool-runner`.
- Run `cargo test --workspace` at the end of the full extraction sequence, or earlier when a slice changes shared public APIs.

## Task-ready Child Task drafts

### 1. Add `spool-runner` and move operation locks

Scope: create the `spool-runner` crate, move Managed Source Repository operation-lock behavior and tests from `spool-cli`, and keep CLI lock commands working through the new crate.

Acceptance Criteria:

- `spool-runner` is a workspace member with only necessary dependencies.
- Operation-lock types/functions are exposed as command-shaped APIs from `spool-runner`.
- `spool-cli` no longer owns operation-lock behavior beyond command UX.
- Existing operation-lock behavior is preserved.

Validation Items:

- `cargo fmt --check`
- Targeted operation-lock tests pass in `spool-runner`.
- Relevant CLI lock command smoke tests pass or remain covered.

### 2. Move Local Worktree Integration

Blocking predecessor: Child Task 1.

Scope: move reusable Local Worktree Integration behavior into `spool-runner` and wire `spool merge retry` through the shared implementation while keeping manual merge UX in `spool-cli`.

Acceptance Criteria:

- Local Worktree Integration behavior lives in `spool-runner`.
- `spool merge retry` and Worker/Supervisor integration callers use the shared runner implementation.
- Manual merge inspection and guidance text remain in `spool-cli`.
- Existing Integration Outcome and state-transition behavior is preserved.

Validation Items:

- `cargo fmt --check`
- `cargo test -p spool-runner`
- Targeted CLI merge retry tests pass.

### 3. Move Agent Launcher and Worker Loop behavior

Blocking predecessor: Child Task 2.

Scope: move Worker Loop orchestration and fake/pi Agent Launcher handling into `spool-runner`, introducing only a small internal launcher interface.

Acceptance Criteria:

- Worker Loop request/outcome APIs are exposed from `spool-runner`.
- FakeLauncher and PiLauncher behavior is separated from Worker Loop orchestration.
- Dynamic launcher plugin loading is not introduced.
- `spool work --once` remains a thin CLI facade with unchanged behavior.

Validation Items:

- `cargo fmt --check`
- `cargo test -p spool-runner`
- Targeted `spool work --once` CLI tests pass.

### 4. Move supervisor orchestration

Blocking predecessor: Child Task 3.

Scope: move supervisor batching/orchestration behavior into `spool-runner` while keeping `spool supervise` CLI parsing and output UX in `spool-cli`.

Acceptance Criteria:

- Supervisor request/outcome APIs are exposed from `spool-runner`.
- Supervisor lock behavior either lives in `spool-runner` or is clearly separated from command UX.
- `spool supervise` remains a thin CLI facade with unchanged behavior.
- Existing retry-due Integrating and worker spawning behavior is preserved.

Validation Items:

- `cargo fmt --check`
- `cargo test -p spool-runner`
- Targeted supervisor CLI tests pass.

### 5. Final CLI test cleanup and runner contribution docs

Blocking predecessor: Child Task 4.

Scope: leave only CLI parsing/output smoke tests in `spool-cli`, ensure runner behavior tests live with `spool-runner`, and document targeted commands for Worker Agents.

Acceptance Criteria:

- Runner behavior tests are no longer stranded in `spool-cli`.
- CLI tests focus on command parsing, command facade wiring, and operator-facing output.
- Runner module documentation or README lists targeted validation commands.
- `docs/RUNNER_EXTRACTION_PLAN.md` reflects the completed structure or points to the new canonical docs.

Validation Items:

- `cargo fmt --check`
- `cargo test -p spool-runner`
- Relevant `cargo test -p spool-cli` targeted tests pass.
- `cargo test --workspace`

## Completed structure

- `spool-runner` owns runner-side behavior and behavior tests for Managed Source Repository operation locks, Local Worktree Delivery, Final Commit metadata, Worker Loop/Agent Launcher handling, and supervisor orchestration.
- `spool-cli` remains a command facade for parsing, command wiring, operator guidance, and output smoke tests.
- Targeted runner validation commands are documented in `crates/spool-runner/README.md`.

## Follow-up after extraction

Create a repository navigation guide for the **Worker Agent Contribution Surface** after the runner crate shape is stable. This is the next simplification priority after `spool-runner`. Organize it domain-first, with sections such as Task Queues and Tasks, Requirements and Workpad, Agent Runs and Claim Leases, Local Worktree Delivery and Integration, CLI Observability, Spool API, and Pi Extension; include a short crate/file index at the end. Each section should point to relevant files, targeted validation commands, and common areas Worker Agents should avoid for that Task type. Do not make this part of the first extraction because the file map will be moving.

Paused design branches to resume after the current changes are committed and the runner extraction Tasks are filed:

- Repository navigation guide details beyond the domain-first shape.
- `spool-server/src/lib.rs` API module simplification.
- `spool-db` persistence test organization.
- `spool-cli` telemetry module simplification.
- Documentation ownership rules for future Worker Agent Tasks.
- Default decomposition patterns for chained Spool Tasks.
- Validation command matrix by domain area.
- Stop conditions for Worker Agents when simplification exposes behavior ambiguity.


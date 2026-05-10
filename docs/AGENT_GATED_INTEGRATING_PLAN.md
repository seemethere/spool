# Agent-Gated Integrating implementation plan

Manual Dogfood Merge is the current delivery bottleneck. The smallest safe path is to add automatic **Local Worktree Delivery** in runner-side code first, then wire it into the **Worker Loop** after a Worker Agent has moved a Task to **Integrating** through existing gates.

## Boundary decision

No ADR change is needed for the first slice. Existing ADRs already require the important boundaries:

- The **Tasker Service** owns Task records, **Task States**, **Delivery Records**, **Integration Outcomes**, and **Audit Events**.
- A runner-side **Delivery Adapter** performs filesystem and Git operations.
- **Local Worktree Delivery** is the only v1 **Delivery Backend**.
- **Integrating** is backend-neutral; local Git merge is only the v1 adapter behavior.

The first implementation should therefore live in worker/CLI-side adapter code, not in `tasker-server` HTTP handlers or `tasker-db` repository functions that shell out to Git.

## First implementation slice

**Slice:** add a runner-side Local Worktree integration adapter and expose it as an explicit command for an already-**Integrating** Task.

Suggested command shape:

```bash
tasker merge integrate <task-identifier>
```

The command is not the final UX; it is a safe, reviewable adapter slice. It replaces most Manual Dogfood Merge shell steps while preserving a narrow boundary: the CLI/worker process runs Git, while Tasker persistence records only state, delivery facts, and outcomes.

### In scope

1. Load the Task, queue delivery config, **Local Worktree** link, and **Task Branch** link.
2. Require the Task to already be **Integrating** so existing structured gates and Review Policy remain authoritative.
3. Run all Git/filesystem checks in a Local Worktree Delivery Adapter module owned by runner-side code.
4. Detect **No-Change Integration** and move the Task to **Done** after recording the outcome.
5. For changed work, perform a **Squash Merge** from the **Task Branch** into the **Main Branch**, create one **Final Commit** with Tasker metadata, record the successful **Integration Outcome**, and move the Task to **Done**.
6. On work-change failures, record a work-change outcome and move the Task to **Rework**.
7. On operational failures, record an operational outcome and leave the Task in **Integrating** for retry.
8. Remove the **Local Worktree** and delete the **Task Branch** after successful integration unless **Done Worktree Retention** is enabled.

### Out of scope for this slice

- Automatically invoking integration from `tasker work --once` after a pi run.
- Adding new Review Session behavior.
- Supporting remote pull requests or non-local delivery backends.
- Adding arbitrary build/test workflow configuration to Tasker.
- Changing structured gate semantics or letting Worker Agents waive requirements.

## Required safety checks

Before mutating the **Managed Source Repository**, the adapter should fail fast unless all checks pass:

1. The configured **Managed Source Repository** is a Git repository and its current branch is the configured **Main Branch**.
2. The **Managed Source Repository** has a clean working tree and clean index.
3. The **Task Branch** is a valid local branch and belongs to the same repository as the **Local Worktree**.
4. The **Local Worktree** exists, is attached to the configured repository, is on the **Task Branch**, and has a clean working tree and clean index.
5. The Task has no pending structured gates; in the first slice this is satisfied by requiring **Integrating**, because transition into **Integrating** already enforces gates.
6. The **Task Branch** includes the current **Main Branch**, or the Task's **Validated Base Commit** equals the current **Main Branch** once that field is implemented.
7. There is no obvious Git lock file in the repository or worktree that would make mutation unsafe.
8. The adapter captures the pre-integration Main Branch commit before any mutating Git command.

## Rollback and failure handling

The adapter should classify failures before changing Task state:

- **Work-Change Delivery Failure**: stale branch/base, uncommitted Local Worktree changes, merge conflict, or validation freshness problems. Record the outcome and transition **Integrating** to **Rework**.
- **Operational Delivery Failure**: missing Git, repository lock, unexpected filesystem error, interrupted process, or inability to record the outcome. Leave the Task in **Integrating** for retry when possible.
- **No-Change Integration**: no diff between **Task Branch** and **Main Branch**. Record `no_changes`, move to **Done**, and clean up delivery artifacts according to retention policy.

For squash merge rollback:

1. Save `pre_merge_head = git rev-parse <main-branch>`.
2. Run the squash merge without committing until the index is prepared.
3. If merge or commit fails before a **Final Commit** is recorded, reset the **Managed Source Repository** back to `pre_merge_head` only after confirming it still points at the expected repository and branch.
4. If Tasker cannot record a success after the Git commit succeeds, leave the Git commit in place and report an operational failure requiring operator repair; do not try to silently rewrite Main Branch history.
5. Cleanup of worktree/branch happens only after the success outcome and **Done** transition have been recorded.

## Follow-up slices

The adapter slice is intentionally not the whole feature. Follow-up Tasks should cover:

1. **TASKER-40**: implement the runner-side Local Worktree integrate command.
2. **TASKER-41**: wire the Worker Loop to call the adapter immediately after a Worker Agent transitions to **Integrating** and still owns the **Claim Lease**.
3. Persist richer delivery records/outcomes and expose them in `tasker task show`, `tasker status`, and `tasker run show` as needed.
4. **TASKER-42**: add or finalize **Validated Base Commit** recording so integration rejects stale validation deterministically.
5. Add a repo-level integration lock or equivalent serialization if queue concurrency allows more than one **Integrating** Task to attempt local merge at the same time; coordinate with the separate Integrating capacity policy work.

## Deterministic test strategy

Use temp SQLite databases and temp Git repositories; real pi remains out of scope.

Targeted tests for the first slice:

- Clean Task Branch with committed changes squash-merges into Main Branch, records success, moves **Integrating** to **Done**, and removes the Local Worktree/Task Branch when retention is false.
- Task Branch with no diff records **No-Change Integration** and moves to **Done** without a merge commit.
- Dirty **Managed Source Repository** produces an operational failure and leaves the Task in **Integrating**.
- Dirty **Local Worktree** produces a work-change failure and moves the Task to **Rework**.
- Stale branch/base produces a work-change failure and moves to **Rework**.
- Merge conflict rolls the Managed Source Repository back to the pre-merge Main Branch commit and records a work-change failure.
- Cleanup failure after a successful merge records enough context for operator repair without losing the **Final Commit**.

Cheap slice checks:

```bash
cargo test -p tasker-cli merge
cargo test -p tasker-db delivery
cargo clippy -p tasker-cli --all-targets -- -D warnings
```

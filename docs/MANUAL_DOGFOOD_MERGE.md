# Manual Dogfood Merge

Manual Dogfood Merge is a temporary dogfooding escape hatch until automatic **Integrating** is implemented. Tasker records **Tasks**, **Agent Runs**, **Task Links**, **Workpad Notes**, **Run Transcripts**, and **Launcher Session Data**; the operator performs final Git inspection and merge outside the **Tasker Service**.

This workflow stays local-first. It is for reviewing completed **Local Worktrees** and integrating them into the **Main Branch** of the **Managed Source Repository** before the Delivery Adapter can do that automatically.

## Managed Source Repository operation lock

Before manually mutating the **Managed Source Repository** during a Manual Dogfood Merge window, acquire the queue-scoped operation lock so supervisors and Worker Loops pause before spawning or claiming new work:

```bash
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data merge lock acquire --queue TASKER --operation manual_integration
```

Check or release it with:

```bash
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data merge lock status --queue TASKER
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data merge lock release --queue TASKER
```

The lock file lives under the active Tasker data directory, records pid/operation/queue/optional Task context, and is scoped by **Task Queue Key**. Manual locks require explicit operator release after the integration window is complete; automatic delivery locks are removed by the process that acquired them, with stale automatic lock cleanup when the recorded process has exited.

## Parallel Local Worktree review checklist

When multiple **Task Branches** are produced in parallel, review them one at a time from the **Managed Source Repository**:

1. Acquire or confirm the **Managed Source Repository** operation lock for the Task Queue before mutating the **Main Branch**.
2. Confirm the **Task Identifier**, title, and current **Task State** for the candidate work.
3. Inspect all **Task Links** and identify the **Local Worktree** path and **Task Branch**.
4. Inspect the latest **Agent Run**, its **Run Transcript**, and any **Launcher Session Data** or failure reason.
5. Read the **Workpad Note** for plan, evidence, handoff notes, and known risks.
6. Verify every **Acceptance Criterion** is satisfied or explicitly handled by the workflow, and every **Validation Item** has current proof.
7. Check the **Local Worktree** for a clean working tree and focused **Task Commits** on the **Task Branch**.
8. Rebase, merge, or otherwise refresh only as an operator Git action outside the **Tasker Service** if the **Main Branch** moved while other Tasks were reviewed.
9. Run the relevant validation from the **Local Worktree** after any refresh.
10. Prefer a squash-style **Local Merge** into the **Main Branch** that produces one **Final Commit** for the Task, then run post-merge validation from the **Managed Source Repository** before marking the batch **Done**.
11. Release the operation lock after the manual integration window is complete.
12. Record final handoff context in Tasker.

Do not batch-merge several **Task Branches** without separately inspecting their Tasker state and validation evidence. Parallel agent execution can produce overlapping changes; each **Local Worktree** needs an independent review against the current **Main Branch**.

## Post-merge batch validation checklist

Individual **Local Worktree** validation is necessary but not sufficient for a Manual Dogfood Merge batch. After each **Local Merge**, or at minimum before marking the merged batch **Done**, validate the combined **Main Branch** from the **Managed Source Repository**. This catches overlapping CLI/API changes where each **Task Branch** passed on its own but the combined **Main Branch** can fail to compile.

Run at least:

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Run extension checks when TypeScript extension files changed in the batch:

```bash
cd extensions/tasker-pi
bun test
bun run build
```

If post-merge validation fails, treat it as unresolved Manual Dogfood Merge work: fix the **Main Branch** before marking affected **Tasks** **Done**, or move the affected **Tasks** back through supported Tasker gates when the work must return to a **Worker Agent**. This checklist is temporary dogfooding guidance and does not replace the target **Integrating** implementation, **Agent-Gated Integration**, or automated **Squash Merge**.

## Inspect the Task and Agent Run

The temporary CLI queue helper lists current **Integrating** **Tasks** and runs only read-only Git inspection commands from each **Local Worktree**:

```bash
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data merge queue --queue TASKER
```

It summarizes **Task Branch**, **Local Worktree**, latest **Agent Run** outcome, structured gate counts, clean worktree status, whether **Task Commits** are present, and whether the Task looks ready for operator merge inspection or needs attention.

The per-Task temporary CLI helper prints a Manual Dogfood Merge inspection plan and also runs only read-only Git inspection commands from the **Local Worktree**:

```bash
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data merge inspect <task-identifier>
```

It summarizes the **Local Worktree**, **Task Branch**, whether the worktree is clean, how the **Task Branch** differs from the **Main Branch**, suggested validation commands, latest **Agent Run**, **Run Transcript**, **Launcher Session Data**, and **Workpad Note** presence. These helpers do not mutate Git state, and any later refresh or merge remains an operator-side action outside the **Tasker Service**. For deeper inspection, use the underlying Tasker reads:

```bash
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data task show <task-identifier>
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data run show <agent-run-id>
```

Check:

- **Task State**, **Acceptance Criteria**, and **Validation Items**
- **Task Links**, especially **Local Worktree** and **Task Branch** entries
- **Agent Runs**, including the latest run outcome and timestamps
- **Run Transcript** path and failure reason, if any
- **Launcher Session Data** captured for the run
- **Workpad Note** handoff, evidence, and follow-up notes
- Read-only Git inspection output: clean/dirty status, diff stats from **Main Branch**, and **Task Commits** since **Main Branch**

If more than one **Agent Run** exists, prefer the latest completed run for handoff evidence, but scan earlier failed or expired runs for unresolved warnings.

## Validate in the Local Worktree

From the **Local Worktree** path recorded on the Task:

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Run extension checks when TypeScript extension files changed:

```bash
cd extensions/tasker-pi
bun test
bun run build
```

Commit focused **Task Branch** changes if the Worker Agent left uncommitted files. A clean **Local Worktree** with committed **Task Commits** is required before any handoff to **Integrating** or manual merge.

## Merge manually or run the runner-side integration helper

For an already-**Integrating** Task, the runner-side Local Worktree Delivery helper can perform the planned v1 **Squash Merge**, record an **Integration Outcome**, move the Task to **Done** or **Rework** as appropriate, and clean up the **Local Worktree**/**Task Branch** after success:

```bash
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data merge integrate <task-identifier>
```

This helper runs in the CLI/worker process, not in the **Tasker Service**. Operational failures leave the Task in **Integrating** for retry; work-change failures such as dirty worktrees, stale branches, or merge conflicts move the Task to **Rework**.

For the remaining fully manual path, from the **Managed Source Repository**, inspect the **Task Branch** against the **Main Branch** and prefer the planned v1 shape: a squash-style **Local Merge** that produces one **Final Commit** containing Tasker metadata such as **Task Identifier**, title, and optionally run ID.

Example operator-side squash integration:

```bash
git switch <main-branch>
git merge --squash <task-branch>
git commit -m "docs: update manual merge guidance (TASKER-60)"
```

Use a concise Conventional Commit subject and include the **Task Identifier** in the subject or body so the **Final Commit** can be traced back to Tasker. Avoid `git merge --no-ff` merge commits for routine Manual Dogfood Merge work unless an operator intentionally needs to preserve branch topology for an exceptional investigation.

After a squash integration, the **Task Branch** is not an ancestor of the **Main Branch**. Do not use branch ancestry as completion proof in the manual path; Tasker database state, **Integration Outcomes**, **Audit Events**, and the **Final Commit** are authoritative for completion and delivery history. This matches the automatic runner-side **Squash Merge** behavior, which also produces one **Final Commit** rather than preserving every **Task Commit** on the **Main Branch**.

Tasker does not perform Git mutations in the **Tasker Service**. During Manual Dogfood Merge, manual Git commands are operator actions performed in the local repository, not hidden Tasker Service behavior.

After manual merge and post-merge validation on the combined **Main Branch**, record a final **Workpad Note** or audit-relevant context through the CLI/API, then request **Task State** transitions only through supported Tasker gates. The temporary confirmation helper only marks an already-merged **Integrating** Task as **Done** when the operator explicitly confirms `--manual`:

```bash
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data merge done <task-identifier> --manual
```

This command performs no Git operations; it records the Task State transition through existing Tasker gates. This procedure is intentionally temporary and does not replace the target **Integrating** implementation, **Agent-Gated Integration**, or automated **Squash Merge**.

# Manual Dogfood Merge

Manual Dogfood Merge is a temporary dogfooding escape hatch until automatic **Integrating** is implemented. Spool records **Tasks**, **Agent Runs**, **Task Links**, **Workpad Notes**, **Run Transcripts**, and **Launcher Session Data**; the operator performs final Git inspection and merge outside the **Spool Service**.

This workflow stays local-first. It is for reviewing completed **Local Worktrees** and integrating them into the **Main Branch** of the **Managed Source Repository** before the Delivery Adapter can do that automatically.

## One-time dogfood state migration

After the Tasker-to-Spool rename lands, migrate the repository dogfood state once from the old `.tasker` local state to `.spool`:

```bash
scripts/migrate-dogfood-state-to-spool.sh
bin/spool-local queue show SPOOL
bin/spool-local status
```

The migration script backs up any existing `.spool` config/database, copies the old SQLite Task Backend with `sqlite3 .backup`, applies pending Spool migrations, copies local Run Transcript storage, and updates the dogfood **Task Queue** to key `SPOOL`, name `Spool Dogfood`, worktree root `.spool/worktrees`, and branch template `spool/{task_identifier}`. Historical **Task Identifiers**, **Task Links**, and old `.tasker/worktrees/...` paths are preserved so completed and canceled history remains inspectable. New dogfood **Tasks** use the `SPOOL` **Task Queue Key** and new Spool local state paths.

Do not run bare old `tasker` commands after migration. Use `bin/spool-local` from the **Managed Source Repository** for project dogfood reads and operator/debug mutations so commands target `.spool/config.toml` and `.spool/data/spool.db`.

## Managed Source Repository operation lock

Before manually mutating the **Managed Source Repository** during a Manual Dogfood Merge window, acquire the queue-scoped operation lock so supervisors and Worker Loops pause before spawning or claiming new work:

```bash
cargo run -p spool-cli -- --config .spool/config.toml --data-dir .spool/data merge lock acquire --queue SPOOL --operation manual_integration
```

Check or release it with:

```bash
cargo run -p spool-cli -- --config .spool/config.toml --data-dir .spool/data merge lock status --queue SPOOL
cargo run -p spool-cli -- --config .spool/config.toml --data-dir .spool/data merge lock release --queue SPOOL
```

The lock file lives under the active Spool data directory, records pid/operation/queue/optional Task context, and is scoped by **Task Queue Key**. Manual locks require explicit operator release after the integration window is complete; automatic delivery locks are removed by the process that acquired them, with stale automatic lock cleanup when the recorded process has exited.

## Parallel Local Worktree review checklist

When multiple **Task Branches** are produced in parallel, review them one at a time from the **Managed Source Repository**:

1. Acquire or confirm the **Managed Source Repository** operation lock for the Task Queue before mutating the **Main Branch**.
2. Confirm the **Task Identifier**, title, and current **Task State** for the candidate work.
3. Inspect all **Task Links** and identify the **Local Worktree** path and **Task Branch**.
4. Inspect the latest **Agent Run**, its **Run Transcript**, and any **Launcher Session Data** or failure reason.
5. Read the **Workpad Note** for plan, evidence, handoff notes, and known risks.
6. Verify every **Acceptance Criterion** is satisfied or explicitly handled by the workflow, and every **Validation Item** has current proof.
7. Check the **Local Worktree** for a clean working tree and focused **Task Commits** on the **Task Branch**.
8. Rebase, merge, or otherwise refresh only as an operator Git action outside the **Spool Service** if the **Main Branch** moved while other Tasks were reviewed.
9. Run the relevant validation from the **Local Worktree** after any refresh.
10. Prefer a squash-style **Local Merge** into the **Main Branch** that produces one **Final Commit** for the Task, then run post-merge validation from the **Managed Source Repository** before marking the batch **Done**.
11. Release the operation lock after the manual integration window is complete.
12. Record final handoff context in Spool.

Do not batch-merge several **Task Branches** without separately inspecting their Spool state and validation evidence. Parallel agent execution can produce overlapping changes; each **Local Worktree** needs an independent review against the current **Main Branch**.

## Post-merge batch validation checklist

Individual **Local Worktree** validation is necessary but not sufficient for a Manual Dogfood Merge batch. After each **Local Merge**, or at minimum before marking the merged batch **Done**, validate the combined **Main Branch** from the **Managed Source Repository**. This catches overlapping CLI/API changes where each **Task Branch** passed on its own but the combined **Main Branch** can fail to compile.

Run at least:

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Run extension checks when TypeScript extension files changed in the batch:

```bash
cd extensions/spool-pi
bun test
bun run build
```

If post-merge validation fails, treat it as unresolved Manual Dogfood Merge work: fix the **Main Branch** before marking affected **Tasks** **Done**, or move the affected **Tasks** back through supported Spool gates when the work must return to a **Worker Agent**. This checklist is temporary dogfooding guidance and does not replace the target **Integrating** implementation, **Agent-Gated Integration**, or automated **Squash Merge**.

## Inspect the Task and Agent Run

The temporary CLI queue helper lists current **Integrating** **Tasks** and runs only read-only Git inspection commands from each **Local Worktree**:

```bash
cargo run -p spool-cli -- --config .spool/config.toml --data-dir .spool/data merge queue --queue SPOOL
```

It summarizes **Task Branch**, **Local Worktree**, latest **Agent Run** outcome, structured gate counts, clean worktree status, whether **Task Commits** are present, and whether the Task looks ready for operator merge inspection or needs attention.

The per-Task temporary CLI helper prints a Manual Dogfood Merge inspection plan and also runs only read-only Git inspection commands from the **Local Worktree**:

```bash
cargo run -p spool-cli -- --config .spool/config.toml --data-dir .spool/data merge inspect <task-identifier>
```

It summarizes the **Local Worktree**, **Task Branch**, whether the worktree is clean, how the **Task Branch** differs from the **Main Branch**, suggested validation commands, latest **Agent Run**, **Run Transcript**, **Launcher Session Data**, and **Workpad Note** presence. These helpers do not mutate Git state, and any later refresh or merge remains an operator-side action outside the **Spool Service**. For deeper inspection, use the underlying Spool reads:

```bash
cargo run -p spool-cli -- --config .spool/config.toml --data-dir .spool/data task show <task-identifier>
cargo run -p spool-cli -- --config .spool/config.toml --data-dir .spool/data run show <agent-run-id>
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
cd extensions/spool-pi
bun test
bun run build
```

Commit focused **Task Branch** changes if the Worker Agent left uncommitted files. A clean **Local Worktree** with committed **Task Commits** is required before any handoff to **Integrating** or manual merge.

## Merge manually or run the runner-side integration helper

For an already-**Integrating** Task, the runner-side Local Worktree Delivery helper can perform the planned v1 **Squash Merge**, record an **Integration Outcome**, move the Task to **Done** or **Rework** as appropriate, and clean up the **Local Worktree**/**Task Branch** after success:

```bash
cargo run -p spool-cli -- --config .spool/config.toml --data-dir .spool/data merge integrate <task-identifier>
```

This helper runs in the CLI/worker process, not in the **Spool Service**. Operational failures leave the Task in **Integrating** for retry; work-change failures such as dirty worktrees, stale branches, or merge conflicts move the Task to **Rework**.

If a retryable **Operational Delivery Failure** has been fixed and the Task is still **Integrating**, retry only Local Worktree Delivery without claiming work or launching a new **Agent Run**:

```bash
cargo run -p spool-cli -- --config .spool/config.toml --data-dir .spool/data merge retry <task-identifier>
```

Use `spool task retry` instead when failed or stuck agent work should return to **Ready**. Use **Rework** for work-change failures unless an operator has explicitly verified a forced delivery retry is safe.

For the remaining fully manual path, from the **Managed Source Repository**, inspect the **Task Branch** against the **Main Branch** and prefer the planned v1 shape: a squash-style **Local Merge** that produces one **Final Commit** with a concise Conventional Commit subject, a compact `Task context` body, and canonical Spool Git trailers. The body should be deterministic, bounded, and derived from safe structured **Task** data: a short **Task Brief** excerpt plus a small number of **Acceptance Criteria** and **Validation Items**. Keep Spool records authoritative; the commit message is only a Git-history aid.

Example body shape:

```text
Task context:
- Brief: <bounded Task Brief excerpt>
- Acceptance 1: <Acceptance Criterion>
- Validation 1: <Validation Item>
```

Keep the trailer block last so `git interpret-trailers --parse` can read it:

```text
Spool-Task: <task-identifier>
Spool-Queue: <task-queue-key>
Spool-Agent-Run: <agent-run-id>
```

`Spool-Agent-Run` may be omitted when the relevant **Agent Run** is unknown. Do not paste raw **Workpad Notes**, **Run Transcripts**, raw **Launcher Session Data** payloads, prompt text, secrets, large free-form **Task** data, or unrelated **Task Queue** data into the **Final Commit** message.

Example operator-side squash integration:

```bash
git switch <main-branch>
git merge --squash <task-branch>
git commit -m "docs: update manual merge guidance" \
  -m "Task context:
- Brief: Clarify the temporary Manual Dogfood Merge path for local-first operator integrations.
- Acceptance 1: Manual Dogfood Merge guidance describes the richer Final Commit message shape.
- Validation 1: Documentation review confirms GitHub and pull requests are not required.

Spool-Task: SPOOL-60
Spool-Queue: SPOOL
Spool-Agent-Run: 5d019294-398e-4f89-ad70-9b434b10dadb"
```

Use `git interpret-trailers --parse` to inspect the trailer block if needed. Avoid `git merge --no-ff` merge commits for routine Manual Dogfood Merge work unless an operator intentionally needs to preserve branch topology for an exceptional investigation.

After a squash integration, the **Task Branch** is not an ancestor of the **Main Branch**. Do not use branch ancestry as completion proof in the manual path; Spool database state, **Integration Outcomes**, **Audit Events**, and the **Final Commit** are authoritative for completion and delivery history. This matches the automatic runner-side **Squash Merge** behavior, which also produces one **Final Commit** rather than preserving every **Task Commit** on the **Main Branch**.

Spool does not perform Git mutations in the **Spool Service**. During Manual Dogfood Merge, manual Git commands are operator actions performed in the local repository, not hidden Spool Service behavior.

After manual merge and post-merge validation on the combined **Main Branch**, record a final **Workpad Note** or audit-relevant context through the CLI/API, then request **Task State** transitions only through supported Spool gates. The temporary confirmation helper only marks an already-merged **Integrating** Task as **Done** when the operator explicitly confirms `--manual`:

```bash
cargo run -p spool-cli -- --config .spool/config.toml --data-dir .spool/data merge done <task-identifier> --manual
```

This command performs no Git operations; it records the Task State transition through existing Spool gates. This procedure is intentionally temporary and does not replace the target **Integrating** implementation, **Agent-Gated Integration**, or automated **Squash Merge**.

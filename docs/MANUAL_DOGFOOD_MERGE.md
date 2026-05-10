# Manual Dogfood Merge

Manual Dogfood Merge is a temporary dogfooding escape hatch until automatic **Integrating** is implemented. Tasker records **Tasks**, **Agent Runs**, **Task Links**, **Workpad Notes**, **Run Transcripts**, and **Launcher Session Data**; the operator performs final Git inspection and merge outside the **Tasker Service**.

This workflow stays local-first. It is for reviewing completed **Local Worktrees** and integrating them into the **Main Branch** of the **Managed Source Repository** before the Delivery Adapter can do that automatically.

## Parallel Local Worktree review checklist

When multiple **Task Branches** are produced in parallel, review them one at a time from the **Managed Source Repository**:

1. Confirm the **Task Identifier**, title, and current **Task State** for the candidate work.
2. Inspect all **Task Links** and identify the **Local Worktree** path and **Task Branch**.
3. Inspect the latest **Agent Run**, its **Run Transcript**, and any **Launcher Session Data** or failure reason.
4. Read the **Workpad Note** for plan, evidence, handoff notes, and known risks.
5. Verify every **Acceptance Criterion** is satisfied or explicitly handled by the workflow, and every **Validation Item** has current proof.
6. Check the **Local Worktree** for a clean working tree and focused **Task Commits** on the **Task Branch**.
7. Rebase, merge, or otherwise refresh only as an operator Git action outside the **Tasker Service** if the **Main Branch** moved while other Tasks were reviewed.
8. Run the relevant validation from the **Local Worktree** after any refresh.
9. Perform the chosen **Local Merge** into the **Main Branch**, then run post-merge validation from the **Managed Source Repository** before marking the batch **Done**.
10. Record final handoff context in Tasker.

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

The temporary CLI helper prints a Manual Dogfood Merge inspection plan and runs only read-only Git inspection commands from the **Local Worktree**:

```bash
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data merge inspect <task-identifier>
```

It summarizes the **Local Worktree**, **Task Branch**, whether the worktree is clean, how the **Task Branch** differs from the **Main Branch**, suggested validation commands, latest **Agent Run**, **Run Transcript**, **Launcher Session Data**, and **Workpad Note** presence. It does not mutate Git state, and any later refresh or merge remains an operator-side action outside the **Tasker Service**. For deeper inspection, use the underlying Tasker reads:

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

## Merge manually

From the **Managed Source Repository**, inspect the **Task Branch** against the **Main Branch** and perform the local merge strategy chosen by the operator. Prefer the planned v1 shape: a squash-style **Local Merge** that produces one **Final Commit** containing Tasker metadata such as **Task Identifier**, title, and optionally run ID.

Tasker does not perform Git mutations in the **Tasker Service**. During Manual Dogfood Merge, Git commands are operator actions performed in the local repository, not hidden Tasker behavior.

After merge and post-merge validation on the combined **Main Branch**, record a final **Workpad Note** or audit-relevant context through the CLI/API, then request **Task State** transitions only through supported Tasker gates. The temporary confirmation helper only marks an already-merged **Integrating** Task as **Done** when the operator explicitly confirms `--manual`:

```bash
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data merge done <task-identifier> --manual
```

This command performs no Git operations; it records the Task State transition through existing Tasker gates. This procedure is intentionally temporary and does not replace the target **Integrating** implementation, **Agent-Gated Integration**, or automated **Squash Merge**.

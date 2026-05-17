# Pre-Dogfooding Development Loop

Spool uses this temporary loop to build Spool until **Dogfooding Cutover**. The loop should stay lightweight and disappear as the primary planning mechanism once real Spool **Tasks** can manage Spool development work.

## Purpose

The **Pre-Dogfooding Development Loop** exists to keep early implementation work small, documented, and testable before Spool can dogfood its own workflow.

## Loop stages

For each **Implementation Slice**:

1. Inspect current docs and code.
2. Propose the next smallest **Implementation Slice** from the current roadmap milestone, including intent, likely files touched, and proposed **Slice Acceptance Checks**.
3. Confirm **Slice Acceptance Checks** with the human.
4. Implement the slice.
5. Run relevant deterministic checks, plus formatting and linting when configured and reasonably cheap.
6. Update documentation when domain language, workflow behavior, persistence meaning, delivery behavior, launcher behavior, or milestone sequencing changes.
7. Run the **Subagent Review Loop** when the slice changes implementation code or other high-impact project behavior.
8. Summarize changes and recommend a Conventional Commit message.
9. Let the human decide whether the agent commits, unless operating inside an **Approved Slice Sequence**.

## Slice selection rules

- Work on one **Implementation Slice** at a time unless the human approves an **Approved Slice Sequence**.
- The agent proposes the next slice; the human approves or redirects it.
- An **Approved Slice Sequence** may contain multiple low-risk slices that the agent implements and commits one-by-one without pausing after each slice.
- Each slice should advance exactly one current roadmap milestone.
- Each slice proposal should include intent, likely files touched, and proposed **Slice Acceptance Checks**.
- For **File-backed Task Creation**, prefer `spool task create --queue <key> --from-file task.md` and start from the canonical template at `.spool/bootstrap-tasks/TEMPLATE.md` so front matter uses valid values for `priority`, `state`, **Acceptance Criteria**, **Validation Items**, tags, review requirement, blockers, and advisory `conflict_hints`. The older `spool task create --bootstrap --queue <key> --file task.md` spelling remains a compatibility path for the temporary dogfooding escape hatch.
- Before filing a batch of related dogfood **Tasks**, classify each relationship as either parallel-ready or sequenced. Parallel-ready Tasks may share `conflict_hints` when they touch overlapping files or documentation areas. Sequenced work must use `blocking_task_identifiers` or another explicit same-queue **Blocking Task** relationship so Spool excludes the dependent **Task** from normal agent pickup until the blocker is **Done**.
- For dependency-aware multi-file **File-backed Task Creation**, use `spool task batch lint --queue <key> --from-file first.md --from-file second.md` before mutation. Give each same-batch Task a unique `batch_key`, and put that key in another file's `blocking_task_keys` when the latter Task is blocked by the former. Use `blocking_task_identifiers` only for already-existing same-queue **Blocking Tasks**. The batch output states dependency direction as `blocked Task -> Blocking Task`, validates missing references and cycles before creation, and creates blockers before blocked Tasks. To keep a related batch from being claimed while still drafting or reviewing it, set `state: backlog`; move Tasks to Ready after the batch graph is correct.
- During dogfooding, Delegating Agents should put likely file paths or documentation areas in structured file-backed `conflict_hints` (aliases: `anticipated_touched_files`, `touched_files`) when creating parallel-ready Tasks. Recommended hotspot names include `crates/spool-db`, `crates/spool-cli`, `worker-loop`, `local-worktree-delivery`, `spool-pi-extension`, `telemetry`, `monitor`, `docs`, and `migrations`; prefer concrete repository paths when known. Operators should inspect `spool status`, `spool monitor --plain`, or `spool task show <task_identifier>` for advisory overlap before starting parallel batches. These hints are a coordination aid only; they do not block claims and are not a full dependency planner. Creation order, shared `conflict_hints`, parent/**Child Task** lineage, and **Workpad Note** or **Task Brief** text also do not block claims; use explicit **Blocking Tasks** for true ordering dependencies.
- Operators can repair an existing **Blocking Task** relationship without raw SQL through `spool task blocker add <blocked_task_identifier> <blocking_task_identifier>`, `spool task blocker remove <blocked_task_identifier> <blocking_task_identifier>`, and `spool task blocker list <task_identifier>`. This tooling is for explicit same-queue **Blocking Tasks** only; it is not a generic dependency planner and does not treat **Child Tasks**, **Task Links**, creation order, advisory `conflict_hints`, or narrative notes as blockers.
- Prefer small, reviewable changes over whole-milestone batches.
- Avoid using Spool **Task** language for pre-dogfooding planning units; use **Implementation Slice** instead.
- Keep implementation single-slice and single-writer by default before dogfooding.
- Parallel agents or sessions may help with research, review, or code auditing, but should not implement separate slices concurrently before Spool provides workflow coordination.
- If a slice discovers extra work that changes scope, architecture, workflow meaning, or acceptance checks, pause and ask whether to expand, split, or defer the work.
- During an **Approved Slice Sequence**, stop for human input when scope, architecture, security, persistence semantics, task lifecycle, delivery behavior, launcher behavior, or unresolved check failures exceed the approved plan.
- Small local fixes that preserve the approved scope may remain inside the current slice.

### File-backed batch examples

Sequenced batch: `api.md` can be created before `cli.md`, but `cli.md` will not be eligible for normal agent pickup until the `api` Task reaches **Done** because it records an explicit same-batch **Blocking Task** relationship.

```yaml
# api.md front matter excerpt
batch_key: api
title: Add API helper
blocking_task_keys: []
```

```yaml
# cli.md front matter excerpt
batch_key: cli
title: Wire CLI to API helper
blocking_task_keys:
  - api
```

```bash
spool task batch lint --queue SPOOL --from-file cli.md --from-file api.md
spool task batch create --queue SPOOL --from-file cli.md --from-file api.md
```

Parallel-ready batch: Tasks may share `conflict_hints` or use different hints, but no dependency is recorded unless `blocking_task_identifiers` or `blocking_task_keys` says so. Use `state: backlog` when you want to stage a reviewed batch before making it claimable.

```yaml
# docs.md front matter excerpt
batch_key: docs
title: Update documentation
state: backlog
conflict_hints:
  - docs
blocking_task_keys: []
```

```yaml
# tests.md front matter excerpt
batch_key: tests
title: Add deterministic tests
state: backlog
conflict_hints:
  - crates/spool-cli
blocking_task_keys: []
```

```bash
spool task batch lint --queue SPOOL --from-file docs.md --from-file tests.md
spool task batch create --queue SPOOL --from-file docs.md --from-file tests.md
```

## Subagent Review Loop

Use advisory subagents to improve pre-dogfooding implementation quality without creating parallel implementation streams.

After **Dogfooding Cutover**, Spool's own development **Tasks** should keep using the same advisory pattern when extra confidence is needed, but the advisory review does not put the **Task** into **Human Review**. For the Spool dogfood **Task Queue**, the default is **Agent-Gated Integration**: once structured **Acceptance Criteria** and **Validation Items** pass, a **Worker Agent** should request **Integrating** unless the **Task** explicitly requires **Human Review** or a human/**Operator** asks for it.

Default loop for implementation slices:

1. Implement the approved **Implementation Slice** as the single writer.
2. Run the slice's deterministic checks.
3. Ask a reviewer subagent to review the uncommitted diff.
4. Fix blocking findings inside the approved scope.
5. Re-run checks after fixes.
6. Re-review when blockers were fixed or the fix materially changes the diff.
7. Commit the slice only after checks pass and no reviewer blockers remain.

Reviewer subagents should focus on correctness, domain language, test coverage, API shape, persistence safety, security/authentication, and whether the diff stays within the approved slice.

A reviewer subagent is an advisory development helper, not Spool's domain **Review Agent**. It does not prepare a **Review Packet** or record a **Review Decision**; those remain part of optional **Review Session** workflows for **Tasks** that actually enter **Human Review**.

### Oracle Escalation

Use an oracle subagent when a reviewer finding, implementation discovery, or user instruction raises a decision conflict or stop-condition ambiguity.

Escalate to oracle for:

- documented architecture contradictions;
- security or authentication decisions;
- persistence semantics or migration safety;
- task lifecycle or state transition rules;
- Local Worktree Delivery behavior;
- launcher/pi behavior;
- domain terminology conflicts;
- whether to split, pause, or continue an **Approved Slice Sequence**.

The oracle decides whether to continue within scope, insert a prerequisite slice, split the work, or stop for human input. After oracle guidance, continue only within the resolved scope.

## Review standard

Use risk-based review before **Dogfooding Cutover**.

Agent self-review is enough for low-risk slices. Human review is required for slices that affect persistence schema, task lifecycle or state transitions, Local Worktree Delivery behavior, launcher/pi behavior, authentication, or ADR-worthy architectural decisions, unless the human explicitly approved those details as part of an **Approved Slice Sequence**.

This pre-dogfooding human-review standard is not the default Spool dogfood workflow. After cutover, Human Review remains an optional product workflow for queues or **Tasks** whose **Review Policy** requires it; ordinary Spool repository work should use **Agent-Gated Integration** plus an advisory **Subagent Review Loop** when useful.

## ADR policy

Create or update an ADR only when a decision is hard to reverse, surprising without context, and trade-off driven. Use existing ADRs and `CONTEXT.md` for ordinary implementation guidance.

## Acceptance check policy

Before implementation begins, define the **Slice Acceptance Checks** for the slice.

Checks should include:

- targeted tests for touched behavior;
- formatting and linting when configured and reasonably cheap;
- documentation updates when behavior or domain meaning changes.

Docs-only slices may have documentation review as their only check.

## Dogfooding Cutover

The first **Dogfooding Cutover** target is after roadmap Milestone 2, when Spool can create and show real development **Tasks** using **File-backed Task Creation**, **Task Queues**, task show/status, **Workpad Notes**, and **Audit Events**.

After cutover, new Spool development work should be represented as real Spool **Tasks** whenever practical, even if execution still relies on manual or partially automated implementation.

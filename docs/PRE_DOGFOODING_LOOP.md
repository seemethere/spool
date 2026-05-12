# Pre-Dogfooding Development Loop

Tasker uses this temporary loop to build Tasker until **Dogfooding Cutover**. The loop should stay lightweight and disappear as the primary planning mechanism once real Tasker **Tasks** can manage Tasker development work.

## Purpose

The **Pre-Dogfooding Development Loop** exists to keep early implementation work small, documented, and testable before Tasker can dogfood its own workflow.

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
- For **File-backed Task Creation**, prefer `tasker task create --queue <key> --from-file task.md` and start from the canonical template at `.tasker/bootstrap-tasks/TEMPLATE.md` so front matter uses valid values for `priority`, `state`, **Acceptance Criteria**, **Validation Items**, tags, review requirement, blockers, and advisory `conflict_hints`. The older `tasker task create --bootstrap --queue <key> --file task.md` spelling remains a compatibility path for the temporary dogfooding escape hatch.
- During dogfooding, Delegating Agents should put likely file paths or documentation areas in structured file-backed `conflict_hints` (aliases: `anticipated_touched_files`, `touched_files`) when creating parallel-ready Tasks. Recommended hotspot names include `crates/tasker-db`, `crates/tasker-cli`, `worker-loop`, `local-worktree-delivery`, `tasker-pi-extension`, `telemetry`, `monitor`, `docs`, and `migrations`; prefer concrete repository paths when known. Operators should inspect `tasker status`, `tasker monitor --plain`, or `tasker task show <task_identifier>` for advisory overlap before starting parallel batches. These hints are a coordination aid only; they do not block claims and are not a full dependency planner.
- Prefer small, reviewable changes over whole-milestone batches.
- Avoid using Tasker **Task** language for pre-dogfooding planning units; use **Implementation Slice** instead.
- Keep implementation single-slice and single-writer by default before dogfooding.
- Parallel agents or sessions may help with research, review, or code auditing, but should not implement separate slices concurrently before Tasker provides workflow coordination.
- If a slice discovers extra work that changes scope, architecture, workflow meaning, or acceptance checks, pause and ask whether to expand, split, or defer the work.
- During an **Approved Slice Sequence**, stop for human input when scope, architecture, security, persistence semantics, task lifecycle, delivery behavior, launcher behavior, or unresolved check failures exceed the approved plan.
- Small local fixes that preserve the approved scope may remain inside the current slice.

## Subagent Review Loop

Use advisory subagents to improve pre-dogfooding implementation quality without creating parallel implementation streams.

After **Dogfooding Cutover**, Tasker's own development **Tasks** should keep using the same advisory pattern when extra confidence is needed, but the advisory review does not put the **Task** into **Human Review**. For the Tasker dogfood **Task Queue**, the default is **Agent-Gated Integration**: once structured **Acceptance Criteria** and **Validation Items** pass, a **Worker Agent** should request **Integrating** unless the **Task** explicitly requires **Human Review** or a human/**Operator** asks for it.

Default loop for implementation slices:

1. Implement the approved **Implementation Slice** as the single writer.
2. Run the slice's deterministic checks.
3. Ask a reviewer subagent to review the uncommitted diff.
4. Fix blocking findings inside the approved scope.
5. Re-run checks after fixes.
6. Re-review when blockers were fixed or the fix materially changes the diff.
7. Commit the slice only after checks pass and no reviewer blockers remain.

Reviewer subagents should focus on correctness, domain language, test coverage, API shape, persistence safety, security/authentication, and whether the diff stays within the approved slice.

A reviewer subagent is an advisory development helper, not Tasker's domain **Review Agent**. It does not prepare a **Review Packet** or record a **Review Decision**; those remain part of optional **Review Session** workflows for **Tasks** that actually enter **Human Review**.

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

This pre-dogfooding human-review standard is not the default Tasker dogfood workflow. After cutover, Human Review remains an optional product workflow for queues or **Tasks** whose **Review Policy** requires it; ordinary Tasker repository work should use **Agent-Gated Integration** plus an advisory **Subagent Review Loop** when useful.

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

The first **Dogfooding Cutover** target is after roadmap Milestone 2, when Tasker can create and show real development **Tasks** using **File-backed Task Creation**, **Task Queues**, task show/status, **Workpad Notes**, and **Audit Events**.

After cutover, new Tasker development work should be represented as real Tasker **Tasks** whenever practical, even if execution still relies on manual or partially automated implementation.

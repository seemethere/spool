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
7. Summarize changes and recommend a Conventional Commit message.
8. Let the human decide whether the agent commits.

## Slice selection rules

- Work on one **Implementation Slice** at a time.
- The agent proposes the next slice; the human approves or redirects it.
- Each slice should advance exactly one current roadmap milestone.
- Each slice proposal should include intent, likely files touched, and proposed **Slice Acceptance Checks**.
- Prefer small, reviewable changes over whole-milestone batches.
- Avoid using Tasker **Task** language for pre-dogfooding planning units; use **Implementation Slice** instead.
- Keep implementation single-slice and single-writer by default before dogfooding.
- Parallel agents or sessions may help with research, review, or code auditing, but should not implement separate slices concurrently before Tasker provides workflow coordination.
- If a slice discovers extra work that changes scope, architecture, workflow meaning, or acceptance checks, pause and ask whether to expand, split, or defer the work.
- Small local fixes that preserve the approved scope may remain inside the current slice.

## Review standard

Use risk-based review before **Dogfooding Cutover**.

Agent self-review is enough for low-risk slices. Human review is required for slices that affect persistence schema, task lifecycle or state transitions, Local Worktree Delivery behavior, launcher/pi behavior, authentication, or ADR-worthy architectural decisions.

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

The first **Dogfooding Cutover** target is after roadmap Milestone 2, when Tasker can create and show real development **Tasks** using **Bootstrap Task Creation**, **Task Queues**, task show/status, **Workpad Notes**, and **Audit Events**.

After cutover, new Tasker development work should be represented as real Tasker **Tasks** whenever practical, even if execution still relies on manual or partially automated implementation.

# Use `tasker review` for local Human Review decisions

Tasker v1 will provide `tasker review <task_identifier>` as the local entry point for Human Review. It launches a Pi-backed Review Agent that reads the Task, Workpad Note, structured completion evidence, Task Links, and Local Worktree diff, prepares a Review Packet, asks the human for an explicit approve/rework decision through the question UI, and records the Review Decision in Tasker without requiring GitHub or a web UI.

## First implementation contract

The smallest useful Review Session implementation is intentionally local-first and CLI-led:

```text
tasker review <task_identifier>
```

The command starts an Interactive Agent Session for exactly one Task. The Task must be in Human Review, and normal Tasker gates still apply: all Acceptance Criteria must be satisfied or waived and all Validation Items must be passed or waived before the Task can leave Human Review for Integrating. The Review Agent is the Actor that records Tasker mutations, while the human provides the decision through the session.

The first command shape should avoid options that imply a second review workflow. Later flags may make packet generation or decision recording separately scriptable, but the default command remains the human-facing local Review Session entry point.

## Review Packet

The Review Agent prepares a concise Review Packet before asking for a decision. The packet is a session artifact, not a Tasker web UI, and should be short enough for local review. It should include:

- Task Identifier, title, current Task State, Task Queue, priority, and whether review is required by Review Policy or explicit agent request.
- Task Brief summary and current Workpad Note handoff summary.
- Acceptance Criteria with Criterion Status and waiver reasons when present.
- Validation Items with Validation Status and validation evidence or waiver reasons when present.
- Relevant Task Links, including the Local Worktree and Task Branch when present.
- Local Worktree summary: current branch, clean/dirty status, Task Commits, and diff summary against the validated base or Main Branch.
- Blocking or follow-up context that affects the human decision, without treating Follow-up Tasks as blockers.
- Any known risks, failed checks, or requested reviewer attention from the Workpad Note.

The packet should not require GitHub, a pull request, or a web dashboard. It may point to local paths and local Git commands because Local Worktree Delivery is the v1 Delivery Backend.

## Review Decision outcomes

The Review Agent asks the human to choose one of two explicit outcomes:

1. **Approve**: record a Review Decision that moves the Task from Human Review to Integrating. The Task remains subject to Local Worktree Delivery rules: the Local Worktree must be clean, work must be committed as Task Commits on the Task Branch, and integration may still produce an Integration Outcome such as Work-Change Delivery Failure or Operational Delivery Failure.
2. **Rework**: record a Review Decision that moves the Task from Human Review to Rework with human feedback. The feedback is captured in Tasker as Review Decision context and summarized in the Workpad Note so a future Worker Agent can revise the existing Local Worktree by default.

A Review Decision must be attributed to the Review Agent Actor and must preserve the human's selected outcome and concise rationale. The Review Agent may also update the Workpad Note with packet summary, decision, feedback, and next-step guidance. Waivers remain explicit Review Agent or Operator mutations and must include reasons; they are not implied by approval.

## Interactive and deterministic boundaries

The first implementation should split behavior so follow-up Tasks can be implemented safely in parallel:

### Deterministic noninteractive helpers

These helpers should be callable without question UI and should have deterministic tests using temp SQLite databases and temp Git repositories where Git state is needed:

- Load the Task, Workpad Note, structured requirements, Task Links, delivery metadata, and relevant Local Worktree state for a Human Review Task.
- Build the Review Packet data model and render a concise text summary.
- Validate that a proposed Review Decision is allowed for the Task State and structured gates.
- Record an approve decision as a State Transition from Human Review to Integrating with Actor attribution and Audit Events.
- Record a rework decision as a State Transition from Human Review to Rework with Actor attribution, feedback, Workpad Note update, and Audit Events.

### Pi-backed interactive behavior

The interactive Review Session uses Pi only for the human-facing review loop:

- Launch a Review Agent through the local `tasker review <task_identifier>` command.
- Present or summarize the Review Packet for the human.
- Ask the human for an explicit approve or rework choice through question UI.
- For rework, ask for concise feedback if the human has not already provided it.
- Call the deterministic decision-recording helper through the Tasker API or Tasker Pi Extension tool surface.

Question UI is expected here because a Review Session is an Interactive Agent Session. The same UI remains invalid for Unattended Worker Sessions.

## Local-first boundaries

This contract deliberately does not introduce a GitHub dependency, pull-request workflow, Tasker web UI, or general review dashboard. Tasker records Tasks, Review Decisions, State Transitions, Workpad Notes, Task Links, Audit Events, and delivery outcomes. Delivery Adapters and local Git tooling remain outside Tasker-owned domain mutation, consistent with Local Worktree Delivery.

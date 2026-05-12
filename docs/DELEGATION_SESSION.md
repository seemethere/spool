# Delegation Sessions

`tasker delegate` starts a Pi-backed **Delegation Session** where a **Delegating Agent** turns out-of-band human intent into one structured **Root Task**. `tasker delegate --refine <task_identifier>` starts the same interactive flow for an existing **Backlog** **Task** and updates that Task until it is ready for agent execution.

The first implementation is local-first and CLI-first. It does not add a web UI, custom workflow fields, external tracker sync, GitHub requirements, or direct human task forms. The deterministic Tasker API helpers should be implemented separately from the Pi-backed interactive session so they can be tested without launching pi.

## CLI contract

### `tasker delegate`

`tasker delegate` is the normal intake path for a new **Root Task**.

1. The command runs from the **Managed Source Repository** with the intended project Tasker config selected.
2. The command starts an **Interactive Agent Session** using the built-in Delegating Agent **Role Prompt**, unless `.tasker/prompts/delegate.md` exists.
3. The **Delegating Agent** runs a one-question-at-a-time **Delegation Interview**.
4. When the task contract is clear, the agent calls deterministic Tasker tooling to create one **Root Task** in the selected **Task Queue**.
5. The created Task defaults to **Backlog** unless the Delegating Agent explicitly requests **Ready** and supplies enough structured requirements for autonomous execution.

The first CLI shape is:

```text
tasker delegate --queue <task_queue_key>
```

If the operator omits `--queue` and exactly one local **Task Queue** is configured, the command may select it. If more than one queue is available, the session should ask the present human which queue to use before creating a Task.

### `tasker delegate --refine <task_identifier>`

`tasker delegate --refine <task_identifier>` refines an existing **Backlog** **Task** instead of creating a new Root Task.

1. Tasker loads the Task, current **Task Brief**, **Acceptance Criteria**, **Validation Items**, priority, tags, **Task Conflict Hints**, **Blocking Tasks**, review requirement, and **Workpad Note**.
2. The **Delegating Agent** interviews the present human only about missing or ambiguous contract details.
3. The agent updates the existing Task through deterministic Tasker tooling.
4. The agent may request a **State Transition** from **Backlog** to **Ready** only after the Task has at least one structured **Acceptance Criterion** and one structured **Validation Item** and is otherwise eligible for **Ready**.

Refinement is only for **Backlog** Tasks in the first implementation. It must not revise active work in **Ready**, **In Progress**, **Human Review**, **Rework**, **Integrating**, **Done**, or **Canceled**; those states use Worker, Review, or Operator flows.

## Delegation Interview behavior

The **Delegation Interview** is a human-present interactive flow. Question UI is expected here, unlike an **Unattended Worker Session**.

The Delegating Agent should:

- ask at most one substantive question at a time;
- stop asking when the Task can be expressed with clear structured requirements;
- read repository context docs such as `CONTEXT.md`, `ROADMAP.md`, and relevant ADRs when needed to use Tasker domain language correctly;
- avoid editing repository files during delegation;
- turn discovered documentation or implementation work into structured requirements, or explicitly note candidate follow-up Tasks rather than making hidden source changes;
- keep the Task small enough for one Worker Agent to execute in a **Local Worktree**;
- prefer **Agent-Gated Integration** by leaving `review_required` false unless the human, Task, or queue policy explicitly requires **Human Review**.

The interview should not collect unsupported v1 planning fields such as due dates, estimates, milestones, custom workflows, assignees, or external tracker metadata.

## Delegating Agent output

The first deterministic creation/refinement payload contains only Tasker-owned structured fields:

- **Task Queue Key**
- title
- **Task Brief** as Markdown narrative context
- priority: `urgent`, `high`, `normal`, or `low`
- initial **Task State**: `backlog` or `ready`
- `review_required`
- tags
- **Task Conflict Hints** as advisory likely paths or documentation areas
- **Blocking Task** identifiers, same-queue only
- ordered **Acceptance Criteria**
- ordered **Validation Items**

The **Task Brief** may include a short "Workpad Note seed" section for narrative handoff, but structured Tasker fields remain authoritative for gates and scheduling. Acceptance Criteria and Validation Items must not be buried only in Markdown.

## Deterministic helper boundary

The first implementation should split the feature into deterministic helpers and the Pi-backed session:

1. A pure normalization/validation helper accepts the Delegating Agent output, normalizes labels, rejects unsupported fields, verifies gate requirements for **Ready**, validates same-queue blockers, and returns a Task creation or refinement command.
2. A Tasker API path persists the Root Task or Backlog refinement with Actor attribution and Audit Events.
3. The CLI path launches pi, supplies the Delegating Agent Role Prompt, and lets the Tasker Pi Extension call the deterministic helpers.

This boundary allows follow-up Tasks to test creation/refinement behavior with temp SQLite databases and fake Delegating Agent output before any real pi smoke test.

## Out of scope for the first implementation

- Creating or updating repository source files during delegation.
- Creating multiple Tasks from one session, except as a later extension once the single-Root Task path is stable.
- Refining non-Backlog Tasks.
- Cross-queue blocking relationships.
- Waiving Acceptance Criteria or Validation Items.
- Replacing File-backed Task Creation internals immediately; file-backed intake remains the dogfooding compatibility path until the Delegation Session is implemented and migrated deliberately.

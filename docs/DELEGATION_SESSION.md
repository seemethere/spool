# Delegation Sessions

The preferred dogfood **Delegation Session** is an ordinary human-present pi session with the **Spool Pi Extension** loaded. The human and **Delegating Agent** clarify out-of-band intent in that session, then the agent calls `spool_create_delegated_root_task` to create one structured **Root Task** through the Spool API. `spool delegate` remains a CLI wrapper/fallback around the same extension-native tooling until a better CLI-hosted interactive loop is deliberately needed.

The first implementation is local-first and CLI-first. It does not add a web UI, custom workflow fields, external tracker sync, GitHub requirements, or direct human task forms. The deterministic Spool API helpers should be implemented separately from the Pi-backed interactive session so they can be tested without launching pi.

For a beginner-facing walkthrough of creating a first structured Root Task, see `docs/TASK_DELEGATION_SESSION_TUTORIAL.md`.

## Extension-native dogfood path

Use this path for ordinary Spool dogfooding instead of writing a bootstrap file or relying on the CLI to orchestrate a Pi RPC interview loop:

1. Start `spool serve` for the project Task Backend and load `extensions/spool-pi/src/index.ts` in a normal human-present pi session with `SPOOL_API_URL`, `SPOOL_API_TOKEN`, and a Delegating Agent actor, for example `SPOOL_ACTOR_KIND=delegating_agent`.
2. Ask the **Delegating Agent** to run a one-question-at-a-time **Delegation Interview** for the intended work.
3. When the draft is clear, the agent calls `spool_create_delegated_root_task` with structured Spool fields.
4. Spool creates one **Root Task** with a **Task Brief**, structured **Acceptance Criteria**, structured **Validation Items**, priority, tags, optional **Task Conflict Hints**, optional same-queue **Blocking Tasks**, review requirement, and Actor-attributed Audit Events.
5. The resulting **Task State** is **Backlog** by default. It may be **Ready** only when the draft includes enough structured requirements for autonomous Worker Agent execution.

Concise dogfood example payload for the extension tool:

```json
{
  "queue_key": "SPOOL",
  "title": "Reduce repeated Spool context reads in Worker prompts",
  "brief": "Update Worker Agent run-start instructions so agents use the Task context bundle before broad discovery and avoid repeated task show/status loops.",
  "priority": "normal",
  "initial_state": "ready",
  "review_required": false,
  "tags": ["dogfood", "agent-efficiency"],
  "conflict_hints": [".spool/prompts", "docs/AGENT_EFFICIENCY_STRATEGY.md"],
  "blocking_task_identifiers": [],
  "acceptance_criteria": [
    "Worker prompt guidance names spool_get_task_context_bundle as the first Spool read",
    "Guidance discourages repeated broad CLI status/show loops during normal Worker execution"
  ],
  "validation_items": [
    "Relevant prompt or documentation tests pass",
    "Documentation includes the updated run-start context discipline"
  ]
}
```

The extension sends this draft to `POST /tasks/delegated-root`; Rust Spool remains authoritative for normalization, validation, persistence, blocking relationships, and Audit Events.

## CLI wrapper/fallback contract

### `spool delegate`

`spool delegate` is a secondary wrapper/fallback intake path for a new **Root Task**. It still launches pi and loads the **Spool Pi Extension**, but the extension-native human-present path above is preferred for dogfooding because the conversation already happens inside pi.

1. The command runs from the **Managed Source Repository** with the intended project Spool config selected.
2. The command starts an **Interactive Agent Session** using the built-in Delegating Agent **Role Prompt**, unless `.spool/prompts/delegate.md` exists.
3. The **Delegating Agent** runs a one-question-at-a-time **Delegation Interview**.
4. When the task contract is clear, the agent calls deterministic Spool tooling to create one **Root Task** in the selected **Task Queue**.
5. The created Task defaults to **Backlog** unless the Delegating Agent explicitly requests **Ready** and supplies enough structured requirements for autonomous execution.

The first dogfoodable CLI shapes are:

```text
spool delegate --queue <task_queue_key> "<initial human intent>"
spool delegate --queue <task_queue_key> --intent-file <path>
```

When using the fallback, run from the **Managed Source Repository** with the project config selected, for example:

```bash
bin/spool-local delegate --queue SPOOL "Investigate and reduce transcript volume regression"
```

When `--pi-extension` is not supplied, `spool delegate` loads the repo-local Spool Pi Extension at `extensions/spool-pi/src/index.ts` if it exists.

Happy path for a human-present Operator or **Delegating Agent**:

1. From the **Managed Source Repository**, run `spool delegate --queue SPOOL "<initial human intent>"` with the project Spool config selected.
2. Spool starts a Pi-backed **Interactive Agent Session** with the Delegating Agent Role Prompt and the Spool Pi Extension environment.
3. The **Delegating Agent** runs the **Delegation Interview**, asking one question at a time and reading local context docs only as needed for Spool domain language.
4. When the draft is clear, the agent validates structured fields and calls the deterministic creation helper, exposed to pi as `spool_create_delegated_root_task`.
5. Spool creates one **Root Task** with a **Task Brief**, structured **Acceptance Criteria**, structured **Validation Items**, priority, tags, optional **Task Conflict Hints**, optional same-queue **Blocking Tasks**, and Actor-attributed Audit Events.
6. The resulting **Task State** is **Backlog** by default. It may be **Ready** only when the draft includes enough structured requirements for autonomous Worker Agent execution.

If the operator omits `--queue` and exactly one local **Task Queue** is configured, the command may select it. If more than one queue is available, the session should ask the present human which queue to use before creating a Task.

### `spool delegate --refine <task_identifier>`

`spool delegate --refine <task_identifier>` refines an existing **Backlog** **Task** instead of creating a new Root Task.

1. Spool loads the Task, current **Task Brief**, **Acceptance Criteria**, **Validation Items**, priority, tags, **Task Conflict Hints**, **Blocking Tasks**, review requirement, and **Workpad Note**.
2. The **Delegating Agent** interviews the present human only about missing or ambiguous contract details.
3. The agent updates the existing Task through deterministic Spool tooling.
4. The agent may request a **State Transition** from **Backlog** to **Ready** only after the Task has at least one structured **Acceptance Criterion** and one structured **Validation Item** and is otherwise eligible for **Ready**.

Refinement is only for **Backlog** Tasks in the first implementation. It must not revise active work in **Ready**, **In Progress**, **Human Review**, **Rework**, **Integrating**, **Done**, or **Canceled**; those states use Worker, Review, or Operator flows.

Happy path for refinement:

1. From the **Managed Source Repository**, run `spool delegate --refine SPOOL-123 "<refinement intent>"` or `spool delegate --refine SPOOL-123 --intent-file intent.md` for an existing **Backlog** Task.
2. Spool loads the Task context bundle and passes the current Task contract, requirements, **Task Conflict Hints**, **Blocking Tasks**, and **Workpad Note** to the Pi-backed **Delegation Session**.
3. The **Delegating Agent** runs a focused **Delegation Interview** about only missing or ambiguous contract details.
4. When the refined contract is clear, the agent validates structured fields and calls the deterministic refinement helper, exposed to pi as `spool_refine_backlog_task`.
5. Spool updates the existing **Backlog** Task, records Actor-attributed Audit Events, preserves requirement status only for unchanged requirements, and resets statuses for clarified requirements.
6. The Task remains **Backlog** unless the Delegating Agent requests **Ready** and the refined Task has at least one structured **Acceptance Criterion** and one structured **Validation Item**.

## Delegation Interview behavior

The **Delegation Interview** is a human-present interactive flow. Question UI is expected here, unlike an **Unattended Worker Session**.

The Delegating Agent should:

- ask at most one substantive question at a time;
- stop asking when the Task can be expressed with clear structured requirements;
- read repository context docs such as `CONTEXT.md`, `ROADMAP.md`, and relevant ADRs when needed to use Spool domain language correctly;
- avoid editing repository files during delegation;
- turn discovered documentation or implementation work into structured requirements, or explicitly note candidate follow-up Tasks rather than making hidden source changes;
- keep the Task small enough for one Worker Agent to execute in a **Local Worktree**;
- prefer **Agent-Gated Integration** by leaving `review_required` false unless the human, Task, or queue policy explicitly requires **Human Review**.

The interview should not collect unsupported v1 planning fields such as due dates, estimates, milestones, custom workflows, assignees, or external tracker metadata.

## Delegating Agent output

The first deterministic creation/refinement payload contains only Spool-owned structured fields:

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

The **Task Brief** may include a short "Workpad Note seed" section for narrative handoff, but structured Spool fields remain authoritative for gates and scheduling. Acceptance Criteria and Validation Items must not be buried only in Markdown.

## Task Link attachment during delegation

During a human-present **Delegation Session**, a **Delegating Agent** should attach **Task Links** through the **Spool Pi Extension** only for references that help a future **Worker Agent**, **Review Agent**, or **Operator** locate collaboration or delivery context. Examples include a local design note, a chat/thread reference, a media artifact supplied by the human, or a non-required external reference. Do not attach links as a substitute for the **Task Brief**, **Acceptance Criteria**, **Validation Items**, **Task Conflict Hints**, or same-queue **Blocking Tasks**.

When a link is the main handoff artifact for the created or refined **Task**, mark it as the **Primary Handoff Link**. Keep examples local-first; GitHub, pull requests, and external trackers may appear only as optional references and must not become required dependencies for the v1 delegation flow.

## Deterministic helper boundary

The first implementation should split the feature into deterministic helpers and the Pi-backed session:

1. A pure normalization/validation helper accepts the Delegating Agent output, normalizes labels, rejects unsupported fields, verifies gate requirements for **Ready**, validates same-queue blockers, and returns a Task creation or refinement command.
2. A Spool API path persists the Root Task or Backlog refinement with Actor attribution and Audit Events.
3. The CLI path launches pi, supplies the Delegating Agent Role Prompt, and lets the Spool Pi Extension call the deterministic helpers.

This boundary allows follow-up Tasks to test creation/refinement behavior with temp SQLite databases and fake Delegating Agent output before any real pi smoke test.

Real pi smoke tests are optional. Normal validation should use deterministic tests with temp SQLite databases, HTTP/API handlers, and fake Pi launchers so local development and CI do not require pi credentials or external agent availability.

## File-backed compatibility path

File-backed Task Creation remains available for dogfooding and compatibility, for example `spool task create --queue <task_queue_key> --from-file task.md`. The older `--bootstrap --file` spelling is the **Bootstrap Task Creation** compatibility path. These commands are useful escape hatches while Spool is being dogfooded, but they are not the preferred long-term intake flow. The preferred dogfood intake path is an extension-native **Delegation Session** that turns human intent into structured Spool fields through `spool_create_delegated_root_task`; `spool delegate` and `spool delegate --refine` remain wrapper/fallback paths around the same deterministic Spool API helpers.

## Out of scope for the first implementation

- Creating or updating repository source files during delegation.
- Creating multiple Tasks from one session, except as a later extension once the single-Root Task path is stable.
- Refining non-Backlog Tasks.
- Cross-queue blocking relationships.
- Waiving Acceptance Criteria or Validation Items.
- Replacing File-backed Task Creation internals immediately; file-backed intake remains the dogfooding compatibility path until the Delegation Session is implemented and migrated deliberately.

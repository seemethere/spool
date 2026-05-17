# First Task Delegation Session tutorial

This tutorial shows how a human-present **Delegation Session** turns vague intent into one structured **Root Task** that an unattended **Worker Agent** can claim later. It is beginner-facing and practical: the goal is not to fill out a human task form, edit repository docs during intake, or invent a custom workflow. The goal is to have a **Delegating Agent** ask one question at a time, read only the context it needs, and create or refine Spool-owned Task data through deterministic tooling.

The preferred path is extension-native:

1. Run `spool serve` for the repository-local **Task Backend**.
2. Open a normal human-present pi session with the **Spool Pi Extension** loaded and `SPOOL_ACTOR_KIND=delegating_agent`.
3. Ask the **Delegating Agent** to run a one-question-at-a-time **Delegation Interview** for your intent.
4. When the contract is clear, the agent calls `spool_create_delegated_root_task` with structured fields.
5. Spool creates one **Root Task** with a **Task Brief**, structured **Acceptance Criteria**, structured **Validation Items**, priority, tags, optional **Task Conflict Hints**, optional same-queue **Blocking Tasks**, `review_required`, and an initial **Task State**.

`spool delegate` is a wrapper/fallback around the same contract. **File-backed Task Creation** with `spool task create --queue <KEY> --from-file task.md` is a compatibility path for cases where the extension-native session is unavailable. Prefer the pi extension path for ordinary dogfood intake because the human and **Delegating Agent** are already in the same interactive session.

## Before you start

Use a repository where Spool has been initialized, a **Task Queue** exists, and the **Spool Service** is running. For a local queue named `APP`, a typical delegation environment looks like this:

```bash
export SPOOL_API_URL=http://127.0.0.1:4317
export SPOOL_API_TOKEN=<token from .spool/config.toml>
export SPOOL_ACTOR_KIND=delegating_agent
export SPOOL_ACTOR_ID=local-delegator
export SPOOL_ACTOR_DISPLAY_NAME="Local Delegating Agent"
```

Load `extensions/spool-pi/src/index.ts` in pi. The extension will call the Spool API directly; the **Delegating Agent** should not shell out to broad CLI commands to create or update the Task.

## Step 1: Start with vague human intent

A good **Delegation Session** can start with an ordinary request:

> I want Spool to prove it can run in this repository. Maybe make a small docs improvement and run a check.

The **Delegating Agent** should not immediately create a Task from this. It should clarify only what is necessary for a future unattended **Worker Agent**.

## Step 2: Use a one-question-at-a-time Delegation Interview

Example interview:

**Delegating Agent:** What outcome would make this first Spool-run Task feel successful?

**Human:** Add a short troubleshooting note to the onboarding docs so first-time users know where to look when `spool work --once` does not claim anything.

**Delegating Agent:** Which docs should the Worker Agent inspect first?

**Human:** Start with `docs/FIRST_RUN_QUICKSTART.md` and `docs/MONITOR.md`; only read other docs if needed.

**Delegating Agent:** Should this need Human Review, or can it use ordinary Agent-Gated Integration after the structured gates pass?

**Human:** No Human Review. If checks pass, it can integrate normally.

At this point the agent has enough information to draft one small Task. It can read `CONTEXT.md`, `ROADMAP.md`, and the named docs if needed to use the project language correctly, but it should not edit files during delegation. If the interview uncovers other work, record it as follow-up Task candidates rather than making hidden source changes.

## Step 3: Draft the structured Task

The **Delegating Agent** should show or internally validate a draft like this before creation:

- **Task Queue Key:** `APP`
- **Title:** `Document no-claim troubleshooting for first Worker Loop run`
- **Priority:** `normal`
- **Initial Task State:** `ready`
- **review_required:** `false`
- **Tags:** `docs`, `onboarding`, `worker-loop`
- **Task Conflict Hints:** `docs/FIRST_RUN_QUICKSTART.md`, `docs/MONITOR.md`
- **Blocking Tasks:** none

**Task Brief:**

```markdown
# Task Brief

## Context

A first-time Spool adopter may run `spool work --once` and see no Task claimed. The current onboarding path should make the common local causes easy to inspect without introducing GitHub, pull requests, or external tracker concepts.

## Requested outcome

Update onboarding documentation with a concise troubleshooting note for a no-claim first Worker Loop run. Keep the guidance local-first and point readers toward existing Spool CLI inspection commands and structured Task readiness checks.

## Workpad Note seed

Focus on `docs/FIRST_RUN_QUICKSTART.md` and `docs/MONITOR.md`. If another doc is a better home, explain the choice in the Workpad Note.
```

**Acceptance Criteria:**

1. `docs/FIRST_RUN_QUICKSTART.md` or another clearly justified onboarding doc explains common reasons `spool work --once` claims no Task.
2. The guidance tells users to verify the Task is **Ready**, the intended **Task Queue** is selected, `spool serve` is reachable, and the **Managed Source Repository** is not blocked by dirty state or an operation lock.
3. The docs keep v1 local-first and do not introduce GitHub, pull requests, assignees, estimates, due dates, or custom workflow fields.

**Validation Items:**

1. `cargo fmt --all -- --check` passes or is explicitly not applicable because only Markdown changed.
2. The changed documentation is reviewed against `CONTEXT.md` terminology.
3. `rg -n "pull request|assignee|estimate|due date" docs/FIRST_RUN_QUICKSTART.md docs/MONITOR.md` does not show new unsupported v1 field guidance.

This draft is small enough for one **Worker Agent**, names likely files without making them scheduling rules, and has structured gates that can be marked satisfied or passed later. The **Task Brief** and **Workpad Note seed** are narrative handoff context; they do not replace structured **Acceptance Criteria** or **Validation Items**.

## Step 4: Create the Root Task with the extension tool

The extension-native creation payload would be:

```json
{
  "queue_key": "APP",
  "title": "Document no-claim troubleshooting for first Worker Loop run",
  "brief": "# Task Brief\n\n## Context\n\nA first-time Spool adopter may run `spool work --once` and see no Task claimed. The current onboarding path should make the common local causes easy to inspect without introducing GitHub, pull requests, or external tracker concepts.\n\n## Requested outcome\n\nUpdate onboarding documentation with a concise troubleshooting note for a no-claim first Worker Loop run. Keep the guidance local-first and point readers toward existing Spool CLI inspection commands and structured Task readiness checks.\n\n## Workpad Note seed\n\nFocus on `docs/FIRST_RUN_QUICKSTART.md` and `docs/MONITOR.md`. If another doc is a better home, explain the choice in the Workpad Note.",
  "priority": "normal",
  "initial_state": "ready",
  "review_required": false,
  "tags": ["docs", "onboarding", "worker-loop"],
  "conflict_hints": ["docs/FIRST_RUN_QUICKSTART.md", "docs/MONITOR.md"],
  "blocking_task_identifiers": [],
  "acceptance_criteria": [
    "docs/FIRST_RUN_QUICKSTART.md or another clearly justified onboarding doc explains common reasons `spool work --once` claims no Task.",
    "The guidance tells users to verify the Task is Ready, the intended Task Queue is selected, `spool serve` is reachable, and the Managed Source Repository is not blocked by dirty state or an operation lock.",
    "The docs keep v1 local-first and do not introduce GitHub, pull requests, assignees, estimates, due dates, or custom workflow fields."
  ],
  "validation_items": [
    "`cargo fmt --all -- --check` passes or is explicitly not applicable because only Markdown changed.",
    "The changed documentation is reviewed against `CONTEXT.md` terminology.",
    "`rg -n \"pull request|assignee|estimate|due date\" docs/FIRST_RUN_QUICKSTART.md docs/MONITOR.md` does not show new unsupported v1 field guidance."
  ]
}
```

The Spool API validates and persists the Task. The pi extension does not duplicate persistence rules, and the Delegating Agent should not add unsupported fields.

## Step 5: Choose Backlog or Ready deliberately

Use **Backlog** when the Task still needs clarification before an unattended **Worker Agent** can safely run. Examples:

- The likely files are unknown and context discovery would be too broad.
- The desired outcome is still subjective or too large.
- There are no structured **Acceptance Criteria** or **Validation Items** yet.
- The Task may be blocked by another same-queue Task that is not recorded.

Use **Ready** only when the Task has enough structured requirements for autonomous execution. A Ready Task should have at least one clear **Acceptance Criterion**, at least one clear **Validation Item**, an appropriate priority, and enough context or **Task Conflict Hints** to avoid broad rediscovery.

Structured Spool fields are authoritative for gates and scheduling: **Task State**, priority, `review_required`, **Blocking Tasks**, **Task Conflict Hints**, **Acceptance Criteria**, and **Validation Items** are the fields that downstream tools evaluate. The **Workpad Note** is narrative handoff context for summaries, risks, validation evidence, and follow-up ideas. Do not bury required gates only in Markdown.

## Step 6: Keep Agent-Gated Integration as the ordinary default

For ordinary local-first Tasks, leave `review_required: false`. After structured **Acceptance Criteria** are satisfied or waived and **Validation Items** are passed or waived, a Worker Agent may request **Integrating** under **Agent-Gated Integration**.

Set `review_required: true` only when the human, Task, or **Task Queue** explicitly requires **Human Review**. Extra confidence can come from an advisory **Subagent Review Loop**, but that is not the same as Spool's domain **Review Session** or **Review Decision**.

## Wrapper and fallback paths

If you cannot run the extension-native session, use the CLI wrapper from the **Managed Source Repository** with the intended project config selected:

```bash
spool delegate --queue APP "Add no-claim troubleshooting guidance for the first Worker Loop run"
```

If pi or the extension is unavailable, use **File-backed Task Creation** as a compatibility escape hatch:

```bash
spool task create --queue APP --from-file task.md
```

These paths should still produce the same Spool-owned fields. They are not invitations to add due dates, estimates, assignees, milestones, custom workflows, GitHub metadata, or pull-request requirements to v1 Tasks.

## Quick checklist for a good first delegated Task

- One small **Root Task**, not a bundle of unrelated work.
- A concise **Task Brief** with context and requested outcome.
- Structured **Acceptance Criteria** that define done.
- Structured **Validation Items** that prove the criteria.
- `priority` is one of `urgent`, `high`, `normal`, or `low`.
- `tags` help categorize but do not change scheduling behavior.
- **Task Conflict Hints** name likely files or docs as advisory context, not required touched files.
- `review_required` is `false` unless **Human Review** is explicitly required.
- Initial **Task State** is **Backlog** when ambiguity remains, or **Ready** when an unattended **Worker Agent** can start.

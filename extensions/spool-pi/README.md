# Spool Pi Extension

Minimal pi extension tools for Worker Agents and human-present Delegation Sessions to update Spool through the HTTP Spool API.

## Configuration

Set these environment variables before loading the extension:

- `SPOOL_API_URL` (default: `http://127.0.0.1:3000`)
- `SPOOL_API_TOKEN` (required)
- `SPOOL_ACTOR_KIND` (default: `worker_agent`)
- `SPOOL_ACTOR_ID` or `SPOOL_ACTOR` (default: `pi-worker`)
- `SPOOL_ACTOR_DISPLAY_NAME` (default: actor id)
- `SPOOL_AGENT_RUN_ID` (optional; used by transition requests and worker status reports when a tool call does not supply one)
- `SPOOL_WORKER_STATUS_PATH` (optional; JSONL path used by `spool supervise` for Worker Agent status reports)

For extension-native dogfood delegation, load the extension in a normal human-present pi session with `SPOOL_ACTOR_KIND=delegating_agent` and an explicit Delegating Agent actor id/display name.

## Tools

- `spool_get_task`
- `spool_get_task_context_bundle` fetches the preferred run-start bundle for Worker Agents: Task Brief, structured requirements, Workpad Note, Task Links, Task Conflict Hints, likely files/path guidance, Task Queue key/config, Local Worktree and Task Branch links, recent Agent Runs with compact normalized session and efficiency summaries, and latest failure/integration summaries. Use this before broad repository discovery instead of repeated `task show`, status, queue, or run lookups; hints are advisory context-planning aids, not authoritative scheduling or completion gates.
- `spool_update_workpad` replaces the Task's singleton Workpad Note body.
- `spool_append_workpad` fetches the current Workpad Note and appends Markdown before saving it, so Worker Agents can add evidence or handoff notes without manually reconstructing the whole note.
- `spool_set_acceptance_criterion_status` accepts `pending`, `satisfied`, or `waived`.
- `spool_set_validation_item_status` accepts `pending`, `passed`, `failed`, or `waived`.
- `spool_create_child_task`
- `spool_attach_task_link` attaches or upserts a typed **Task Link** with Task Identifier, kind, target, optional label, and optional Primary Handoff Link selection. **Task Links** are collaboration/delivery references, not authoritative **Acceptance Criteria**, **Validation Items**, or scheduling gates.
- `spool_create_delegated_root_task` creates one Root Task from structured Delegation Session draft data by calling Spool's deterministic `/tasks/delegated-root` API path. The Rust Spool API validates the draft and persists the Task; the extension does not duplicate persistence rules.
- `spool_refine_backlog_task` refines an existing Backlog Task through the deterministic refinement API path.
- `spool_request_transition` accepts the fixed v1 Task State values.
- `spool_record_review_decision` records a human Review Decision through the deterministic review API path.
- `spool_report_worker_status` records supervisor-only status (`completion_intent`, `blocked`, or `retryable_failure`) without changing Spool state.

All mutations send explicit Spool Actor attribution. The extension does not shell out to the Spool CLI. Existing replace-style Workpad Note updates remain available for callers that need to rewrite the full note. Worker status reports are a local supervisor contract, not authoritative Spool state. The context bundle is read-only and intentionally excludes raw Run Transcript bodies, raw launcher payloads, secrets, and unrelated queue data.


## Task Link attachment guidance

Attach **Task Links** through the Spool Pi Extension when a reference helps another agent, a **Review Session**, or Local Worktree Delivery find the right artifact without scraping transcripts or guessing paths. Common local-first examples:

- Worker Agents may attach `local_worktree` and `task_branch` references when preparing Local Worktree Delivery, and may attach local patch, log, or artifact references when they are useful handoff evidence.
- Delegating Agents may attach non-gate context from an Interactive Agent Session, such as a local design note, chat/thread reference, media artifact, or external reference supplied by the human. Keep the Task Brief and structured requirements authoritative; do not hide Acceptance Criteria or Validation Items in links.
- Review Agents may attach or update review packet/artifact references that help future Worker Agents or Operators inspect feedback, while recording the actual Review Decision through the review-decision path.

Use `is_primary: true` only for the main handoff artifact a finishing or reviewing agent should inspect first. Task Links are collaboration and delivery references; they are not scheduling rules, **Review Policy**, **Criterion Status**, **Validation Status**, **Waivers**, or proof that a Task can transition. GitHub, pull requests, and external trackers are optional future/reference targets, not required dependencies for v1.

## Worker Loop usage

`spool work --launcher pi` starts `pi --mode rpc` with these environment variables set for the fresh Agent Run. Run `spool serve` separately so the extension can reach the Spool API, then inspect the Agent Run with `spool run show <agent-run-id>`.

## Dogfood delegation usage

Preferred ordinary dogfood intake happens inside a human-present pi session: clarify intent with the Delegating Agent, then call `spool_create_delegated_root_task` with structured fields such as:

```json
{
  "queue_key": "SPOOL",
  "title": "Document extension-native dogfood delegation",
  "brief": "Update Spool docs so ordinary dogfood delegation uses a human-present pi session and Spool Pi Extension tooling instead of bootstrap files.",
  "priority": "normal",
  "initial_state": "ready",
  "review_required": false,
  "tags": ["dogfood", "delegation-sessions"],
  "conflict_hints": ["docs/DELEGATION_SESSION.md", "extensions/spool-pi"],
  "blocking_task_identifiers": [],
  "acceptance_criteria": ["Docs name the extension-native path as preferred dogfood intake"],
  "validation_items": ["Extension contract tests pass"]
}
```

File-backed Task Creation and `spool delegate` remain fallback or wrapper paths.

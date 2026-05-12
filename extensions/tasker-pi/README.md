# Tasker Pi Extension

Minimal pi extension tools for Worker Agents and human-present Delegation Sessions to update Tasker through the HTTP Tasker API.

## Configuration

Set these environment variables before loading the extension:

- `TASKER_API_URL` (default: `http://127.0.0.1:3000`)
- `TASKER_API_TOKEN` (required)
- `TASKER_ACTOR_KIND` (default: `worker_agent`)
- `TASKER_ACTOR_ID` or `TASKER_ACTOR` (default: `pi-worker`)
- `TASKER_ACTOR_DISPLAY_NAME` (default: actor id)
- `TASKER_AGENT_RUN_ID` (optional; used by transition requests and worker status reports when a tool call does not supply one)
- `TASKER_WORKER_STATUS_PATH` (optional; JSONL path used by `tasker supervise` for Worker Agent status reports)

For extension-native dogfood delegation, load the extension in a normal human-present pi session with `TASKER_ACTOR_KIND=delegating_agent` and an explicit Delegating Agent actor id/display name.

## Tools

- `tasker_get_task`
- `tasker_get_task_context_bundle` fetches the preferred run-start bundle for Worker Agents: Task Brief, structured requirements, Workpad Note, Task Links, Task Conflict Hints, likely files/path guidance, Task Queue key/config, Local Worktree and Task Branch links, recent Agent Runs with compact normalized session and efficiency summaries, and latest failure/integration summaries. Use this before broad repository discovery instead of repeated `task show`, status, queue, or run lookups; hints are advisory context-planning aids, not authoritative scheduling or completion gates.
- `tasker_update_workpad` replaces the Task's singleton Workpad Note body.
- `tasker_append_workpad` fetches the current Workpad Note and appends Markdown before saving it, so Worker Agents can add evidence or handoff notes without manually reconstructing the whole note.
- `tasker_set_acceptance_criterion_status` accepts `pending`, `satisfied`, or `waived`.
- `tasker_set_validation_item_status` accepts `pending`, `passed`, `failed`, or `waived`.
- `tasker_create_child_task`
- `tasker_create_delegated_root_task` creates one Root Task from structured Delegation Session draft data by calling Tasker's deterministic `/tasks/delegated-root` API path. The Rust Tasker API validates the draft and persists the Task; the extension does not duplicate persistence rules.
- `tasker_refine_backlog_task` refines an existing Backlog Task through the deterministic refinement API path.
- `tasker_request_transition` accepts the fixed v1 Task State values.
- `tasker_record_review_decision` records a human Review Decision through the deterministic review API path.
- `tasker_report_worker_status` records supervisor-only status (`completion_intent`, `blocked`, or `retryable_failure`) without changing Tasker state.

All mutations send explicit Tasker Actor attribution. The extension does not shell out to the Tasker CLI. Existing replace-style Workpad Note updates remain available for callers that need to rewrite the full note. Worker status reports are a local supervisor contract, not authoritative Tasker state. The context bundle is read-only and intentionally excludes raw Run Transcript bodies, raw launcher payloads, secrets, and unrelated queue data.

## Worker Loop usage

`tasker work --launcher pi` starts `pi --mode rpc` with these environment variables set for the fresh Agent Run. Run `tasker serve` separately so the extension can reach the Tasker API, then inspect the Agent Run with `tasker run show <agent-run-id>`.

## Dogfood delegation usage

Preferred ordinary dogfood intake happens inside a human-present pi session: clarify intent with the Delegating Agent, then call `tasker_create_delegated_root_task` with structured fields such as:

```json
{
  "queue_key": "TASKER",
  "title": "Document extension-native dogfood delegation",
  "brief": "Update Tasker docs so ordinary dogfood delegation uses a human-present pi session and Tasker Pi Extension tooling instead of bootstrap files.",
  "priority": "normal",
  "initial_state": "ready",
  "review_required": false,
  "tags": ["dogfood", "delegation-sessions"],
  "conflict_hints": ["docs/DELEGATION_SESSION.md", "extensions/tasker-pi"],
  "blocking_task_identifiers": [],
  "acceptance_criteria": ["Docs name the extension-native path as preferred dogfood intake"],
  "validation_items": ["Extension contract tests pass"]
}
```

File-backed Task Creation and `tasker delegate` remain fallback or wrapper paths.

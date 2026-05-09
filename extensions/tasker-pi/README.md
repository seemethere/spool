# Tasker Pi Extension

Minimal pi extension tools for Worker Agents to update Tasker through the HTTP Tasker API.

## Configuration

Set these environment variables before loading the extension:

- `TASKER_API_URL` (default: `http://127.0.0.1:3000`)
- `TASKER_API_TOKEN` (required)
- `TASKER_ACTOR_KIND` (default: `worker_agent`)
- `TASKER_ACTOR_ID` or `TASKER_ACTOR` (default: `pi-worker`)
- `TASKER_ACTOR_DISPLAY_NAME` (default: actor id)
- `TASKER_AGENT_RUN_ID` (optional; used by transition requests when a tool call does not supply one)

## Tools

- `tasker_get_task`
- `tasker_update_workpad` replaces the Task's singleton Workpad Note body.
- `tasker_append_workpad` fetches the current Workpad Note and appends Markdown before saving it, so Worker Agents can add evidence or handoff notes without manually reconstructing the whole note.
- `tasker_set_acceptance_criterion_status` accepts `pending`, `satisfied`, or `waived`.
- `tasker_set_validation_item_status` accepts `pending`, `passed`, `failed`, or `waived`.
- `tasker_create_child_task`
- `tasker_request_transition` accepts the fixed v1 Task State values.

All mutations send explicit Tasker Actor attribution. The extension does not shell out to the Tasker CLI. Existing replace-style Workpad Note updates remain available for callers that need to rewrite the full note.

## Worker Loop usage

`tasker work --launcher pi` starts `pi --mode rpc` with these environment variables set for the fresh Agent Run. Run `tasker serve` separately so the extension can reach the Tasker API, then inspect the Agent Run with `tasker run show <agent-run-id>`.

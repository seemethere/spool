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
- `tasker_update_workpad`
- `tasker_set_acceptance_criterion_status`
- `tasker_set_validation_item_status`
- `tasker_create_child_task`
- `tasker_request_transition`

All mutations send explicit Tasker Actor attribution. The extension does not shell out to the Tasker CLI.

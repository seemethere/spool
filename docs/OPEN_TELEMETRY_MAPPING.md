# Future OpenTelemetry mapping for Tasker telemetry

This design note maps Tasker's local telemetry model to future OpenTelemetry (OTel) traces, spans, metrics, and events. It does not add an exporter, collector, network client, or external telemetry dependency. Tasker remains local-first: external telemetry export is future optional behavior, disabled by default, and must require an explicit Operator configuration before anything leaves the local machine.

## Goals

- Keep current Tasker records authoritative: **Audit Events**, **Agent Runs**, **Launcher Session Data**, **Integration Outcomes**, and current Task rows remain the source for **Workflow Metrics**.
- Define a stable mapping that an optional future exporter can derive from stored local data.
- Preserve Tasker language in exported names and attributes so OTel data remains understandable without becoming a separate workflow model.
- Avoid exporting **Run Transcripts**, raw launcher payloads, Task Brief text, Workpad Note text, prompts, tool arguments, file contents, API tokens, or secrets.

## Non-goals

- Implementing OTel export in this slice.
- Sending telemetry to a collector by default.
- Replacing Tasker persistence with an observability backend.
- Making OTel spans the authoritative history; **Audit Events** remain the append-only history and current relational rows remain the read model.

## Resource mapping

A future exporter should attach resource attributes that describe the local Tasker deployment without exposing sensitive paths unless explicitly allowed.

| Tasker concept | OTel resource attribute | Notes |
| --- | --- | --- |
| Tasker Service | `service.name = "tasker"` | Stable service identity. |
| Tasker version | `service.version` | From the CLI/service version. |
| Task Queue Key | `tasker.queue.key` | Low-cardinality queue identifier such as `TASKER`. |
| Managed Source Repository | `tasker.repository.id` | Prefer a local stable hash or configured alias, not the raw path by default. |
| Main Branch | `tasker.repository.main_branch` | Export only branch name, not remotes. |
| Worker Loop | `tasker.worker.id` | Local worker/supervisor identifier when available. |

## Trace model

Tasker can derive traces from persisted workflow records. Trace IDs should be exporter-generated and stable per export run only if no canonical trace ID exists; Tasker should not store OTel trace state as authoritative workflow state.

### Task lifecycle trace

A future exporter may emit one trace per **Task**. The trace represents the Task's end-to-end workflow across state changes, Agent Runs, validation, delivery, and terminal state.

Recommended root span:

- Span name: `tasker.task`
- Span kind: internal
- Start time: Task creation time
- End time: terminal state time when the Task reaches **Done** or **Canceled**; otherwise unset for streaming export or set to the export snapshot time for batch reports
- Attributes:
  - `tasker.task.identifier`
  - `tasker.task.state`
  - `tasker.queue.key`
  - `tasker.priority`
  - `tasker.review_required`
  - `tasker.has_blocking_tasks`
  - `tasker.validated_base_commit.recorded` as a boolean, not the commit by default

State changes should be represented as events on the Task span and, when useful for duration analysis, as child spans named `tasker.task.state.<state>`. The child-span form lets operators see time spent in **Ready**, **In Progress**, **Human Review**, **Rework**, **Integrating**, and terminal states without treating Task States as separate Tasks.

### Agent Run trace/span

Each **Agent Run** maps to a child span of the Task trace.

- Span name: `tasker.agent_run`
- Start time: claim time
- End time: finish, cancellation, failure, or expiry time
- Attributes:
  - `tasker.agent_run.id`
  - `tasker.agent_run.outcome`
  - `tasker.agent_run.lease_expired`
  - `tasker.launcher.kind`
  - `tasker.launcher.session_id` when safe and available
  - `tasker.launcher.model` and `tasker.launcher.provider` when available
  - `tasker.unattended_question_detected`
  - `tasker.blocking_ui_detected`

**Lease Heartbeats** should normally be metrics or summarized events, not one span per heartbeat. Exporters may add an event for lease expiry or abnormal heartbeat gaps.

### Local Worktree setup span

Local Worktree Delivery setup should be a child span of the Task trace or the Agent Run span that performed setup.

- Span name: `tasker.local_worktree.setup`
- Attributes:
  - `tasker.delivery.backend = "local_worktree"`
  - `tasker.task_branch`
  - `tasker.worktree.created`
  - `tasker.worktree.reused`
  - `tasker.worktree.path.exported = false` by default

Do not export raw **Local Worktree** paths by default. If operators explicitly enable local path export for their own machine, use an attribute such as `tasker.worktree.path` and document the privacy tradeoff.

### Validation spans and events

Structured requirement evidence should be visible without exporting narrative content.

- Span name: `tasker.validation`
- Events:
  - `tasker.acceptance_criterion.status_changed`
  - `tasker.validation_item.status_changed`
- Attributes:
  - `tasker.requirement.kind = "acceptance_criterion" | "validation_item"`
  - `tasker.requirement.position`
  - `tasker.requirement.status`
  - `tasker.validated_base_commit.matches_main` when known

Do not export Acceptance Criterion text, Validation Item text, waiver text, or Workpad Note content by default. Counts and statuses are enough for workflow telemetry.

### Integration attempt span

Each Local Worktree Delivery attempt maps to a child span of the Task trace.

- Span name: `tasker.integration_attempt`
- Start time: attempt start
- End time: outcome recorded
- Attributes:
  - `tasker.delivery.backend`
  - `tasker.integration.outcome = "success" | "no_changes" | "work_change_failure" | "operational_failure"`
  - `tasker.integration.strategy = "squash_merge"` for v1 successful changed work
  - `tasker.integration.final_commit.recorded` as a boolean, not the commit by default
  - `tasker.integration.cleaned_worktree`
  - `tasker.integration.deleted_task_branch`

Merge conflicts, stale validation, dirty worktrees, repository locks, and cleanup failures should be span events with sanitized reason codes. Detailed Git output should stay local unless an Operator explicitly enables diagnostic payload export.

### Supervisor and Worker Loop events

Supervisor and Worker Loop observability should favor metrics and events over long-running spans.

Recommended events:

- `tasker.supervisor.started`
- `tasker.supervisor.exited`
- `tasker.supervisor.lock_acquired`
- `tasker.supervisor.lock_stale_removed`
- `tasker.worker.claim_attempted`
- `tasker.worker.claim_succeeded`
- `tasker.worker.no_eligible_task`
- `tasker.worker.run_timed_out`

Recommended attributes:

- `tasker.queue.key`
- `tasker.worker.concurrency`
- `tasker.queue.concurrency_limit`
- `tasker.supervisor.watch`
- `tasker.supervisor.allow_overlap`

## Metrics mapping

A future exporter should derive OTel metrics from Tasker records rather than write a separate metrics source of truth.

| Workflow Metric | OTel instrument | Suggested name | Dimensions |
| --- | --- | --- | --- |
| Task count by state | Observable gauge | `tasker.tasks` | `tasker.queue.key`, `tasker.task.state`, `tasker.priority` |
| Task lifecycle duration | Histogram | `tasker.task.duration` | `tasker.queue.key`, terminal state, priority |
| Time in Task State | Histogram | `tasker.task.state.duration` | `tasker.queue.key`, state |
| Agent Run duration | Histogram | `tasker.agent_run.duration` | `tasker.queue.key`, launcher kind, outcome |
| Agent Run outcomes | Counter | `tasker.agent_run.outcomes` | `tasker.queue.key`, launcher kind, outcome |
| Claim Lease expiries | Counter | `tasker.claim_lease.expiries` | `tasker.queue.key`, launcher kind |
| Retry holds | Observable gauge or counter | `tasker.retry_holds` | `tasker.queue.key`, reason code |
| Requirement status changes | Counter | `tasker.requirement.status_changes` | requirement kind, from status, to status |
| Validation pass/fail counts | Counter | `tasker.validation.results` | validation status, queue |
| Integration outcomes | Counter | `tasker.integration.outcomes` | delivery backend, outcome |
| Human Review wait time | Histogram | `tasker.human_review.duration` | queue, review decision when known |
| Queue throughput | Counter | `tasker.queue.completed_tasks` | queue, terminal state |
| Transcript artifact size | Histogram or gauge | `tasker.run_transcript.bytes` | launcher kind, queue |
| Token usage | Counter | `tasker.launcher.tokens` | launcher kind, provider, model, token kind |
| Cost totals | Counter | `tasker.launcher.cost` | launcher kind, provider, model, currency |
| Tool-call counts | Counter | `tasker.launcher.tool_calls` | launcher kind, tool kind when safe |

Token and cost metrics should be emitted only when present in **Launcher Session Data**. They should not imply an external billing source of truth.

## Event mapping

**Audit Events** map naturally to OTel span events or log records. The future exporter should prefer sanitized event names and structured attributes over payload dumps.

Examples:

| Audit/Event source | OTel event/log name | Sanitized attributes |
| --- | --- | --- |
| Task created | `tasker.task.created` | task identifier, queue, priority |
| State Transition requested/applied | `tasker.task.state_transition` | from state, to state, actor kind |
| Claim created | `tasker.agent_run.claimed` | agent run id, queue, launcher kind |
| Lease heartbeat gap/expiry | `tasker.claim_lease.expired` | agent run id, seconds since heartbeat |
| Workpad Note updated | `tasker.workpad.updated` | revision number or content length only |
| Requirement status set | `tasker.requirement.status_changed` | kind, position, status |
| Child Task created | `tasker.child_task.created` | parent identifier, child identifier, blocking boolean |
| Integration Outcome recorded | `tasker.integration.outcome_recorded` | delivery backend, outcome |
| Cleanup run inspected/deleted artifacts | `tasker.cleanup.runs` | dry-run/delete flag, counts, bytes |

## Privacy and local-first rules

A future OTel exporter must obey these rules:

1. Disabled by default. Operators must explicitly configure export.
2. Local records remain complete enough for CLI-first observability when export is disabled.
3. Do not export Run Transcripts, raw launcher JSON, prompts, Workpad Note text, Task Brief text, requirement text, tool arguments, environment variables, file contents, API tokens, or secret-looking values by default.
4. Prefer identifiers, statuses, reason codes, counts, durations, and byte sizes.
5. Treat repository paths, branch names, model names, and session IDs as potentially sensitive; export only the minimum needed and allow redaction or hashing.
6. Keep OTel export failure operationally isolated from Task execution. Export failure must not fail an Agent Run, State Transition, validation update, or integration attempt.
7. Document every opt-in diagnostic expansion that can reveal local paths or content.

## Future implementation outline

If Tasker later adds optional OTel export, the smallest safe path is:

1. Add a read-only exporter command or background task that derives telemetry from existing Tasker data.
2. Start with metrics and sanitized logs/events before adding traces.
3. Use explicit Operator configuration for endpoint, protocol, headers, resource labels, and redaction policy.
4. Keep exporter state, retries, and failures separate from Tasker workflow transactions.
5. Add deterministic tests using a fake OTel collector or in-memory exporter; keep real collector smoke tests opt-in.

No follow-up implementation Task is required by this mapping alone. Concrete follow-up Tasks should be created only when Tasker is ready to implement optional export configuration, a read-only exporter, or additional sanitized metrics that current local telemetry cannot derive.

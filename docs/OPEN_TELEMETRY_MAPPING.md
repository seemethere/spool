# Future OpenTelemetry mapping for Spool telemetry

This design note maps Spool's local telemetry model to future OpenTelemetry (OTel) traces, spans, metrics, and events. It does not add an exporter, collector, network client, or external telemetry dependency. Spool remains local-first: external telemetry export is future optional behavior, disabled by default, and must require an explicit Operator configuration before anything leaves the local machine.

## Goals

- Keep current Spool records authoritative: **Audit Events**, **Agent Runs**, **Launcher Session Data**, **Integration Outcomes**, and current Task rows remain the source for **Workflow Metrics**.
- Define a stable mapping that an optional future exporter can derive from stored local data.
- Preserve Spool language in exported names and attributes so OTel data remains understandable without becoming a separate workflow model.
- Avoid exporting **Run Transcripts**, raw launcher payloads, Task Brief text, Workpad Note text, prompts, tool arguments, file contents, API tokens, or secrets.

## Non-goals

- Implementing OTel export in this slice.
- Sending telemetry to a collector by default.
- Replacing Spool persistence with an observability backend.
- Making OTel spans the authoritative history; **Audit Events** remain the append-only history and current relational rows remain the read model.

## Resource mapping

A future exporter should attach resource attributes that describe the local Spool deployment without exposing sensitive paths unless explicitly allowed.

| Spool concept | OTel resource attribute | Notes |
| --- | --- | --- |
| Spool Service | `service.name = "spool"` | Stable service identity. |
| Spool version | `service.version` | From the CLI/service version. |
| Task Queue Key | `spool.queue.key` | Low-cardinality queue identifier such as `SPOOL`. |
| Managed Source Repository | `spool.repository.id` | Prefer a local stable hash or configured alias, not the raw path by default. |
| Main Branch | `spool.repository.main_branch` | Export only branch name, not remotes. |
| Worker Loop | `spool.worker.id` | Local worker/supervisor identifier when available. |

## Trace model

Spool can derive traces from persisted workflow records. Trace IDs should be exporter-generated and stable per export run only if no canonical trace ID exists; Spool should not store OTel trace state as authoritative workflow state.

### Task lifecycle trace

A future exporter may emit one trace per **Task**. The trace represents the Task's end-to-end workflow across state changes, Agent Runs, validation, delivery, and terminal state.

Recommended root span:

- Span name: `spool.task`
- Span kind: internal
- Start time: Task creation time
- End time: terminal state time when the Task reaches **Done** or **Canceled**; otherwise unset for streaming export or set to the export snapshot time for batch reports
- Attributes:
  - `spool.task.identifier`
  - `spool.task.state`
  - `spool.queue.key`
  - `spool.priority`
  - `spool.review_required`
  - `spool.has_blocking_tasks`
  - `spool.validated_base_commit.recorded` as a boolean, not the commit by default

State changes should be represented as events on the Task span and, when useful for duration analysis, as child spans named `spool.task.state.<state>`. The child-span form lets operators see time spent in **Ready**, **In Progress**, **Human Review**, **Rework**, **Integrating**, and terminal states without treating Task States as separate Tasks.

### Agent Run trace/span

Each **Agent Run** maps to a child span of the Task trace.

- Span name: `spool.agent_run`
- Start time: claim time
- End time: finish, cancellation, failure, or expiry time
- Attributes:
  - `spool.agent_run.id`
  - `spool.agent_run.outcome`
  - `spool.agent_run.lease_expired`
  - `spool.launcher.kind`
  - `spool.launcher.session_id` when safe and available
  - `spool.launcher.model` and `spool.launcher.provider` when available
  - `spool.unattended_question_detected`
  - `spool.blocking_ui_detected`

`spool.blocking_ui_detected` is true only when an unattended **Worker Loop** observes a blocking pi extension UI request such as `confirm`, `input`, `select`, or `editor`. Fire-and-forget extension UI events such as `notify` are benign and should not set the attribute.

`spool.unattended_question_detected` is true when launcher metadata explicitly reports an unattended question, or when a question event is observed on a non-successful Agent Run. Successful runs with benign extension UI or other non-blocking launcher events should not be labeled as unexpected questions.

**Lease Heartbeats** should normally be metrics or summarized events, not one span per heartbeat. Exporters may add an event for lease expiry or abnormal heartbeat gaps.

### Local Worktree setup span

Local Worktree Delivery setup should be a child span of the Task trace or the Agent Run span that performed setup.

- Span name: `spool.local_worktree.setup`
- Attributes:
  - `spool.delivery.backend = "local_worktree"`
  - `spool.task_branch`
  - `spool.worktree.created`
  - `spool.worktree.reused`
  - `spool.worktree.path.exported = false` by default

Do not export raw **Local Worktree** paths by default. If operators explicitly enable local path export for their own machine, use an attribute such as `spool.worktree.path` and document the privacy tradeoff.

### Validation spans and events

Structured requirement evidence should be visible without exporting narrative content.

- Span name: `spool.validation`
- Events:
  - `spool.acceptance_criterion.status_changed`
  - `spool.validation_item.status_changed`
- Attributes:
  - `spool.requirement.kind = "acceptance_criterion" | "validation_item"`
  - `spool.requirement.position`
  - `spool.requirement.status`
  - `spool.validated_base_commit.matches_main` when known

Do not export Acceptance Criterion text, Validation Item text, waiver text, or Workpad Note content by default. Counts and statuses are enough for workflow telemetry.

### Integration attempt span

Each Local Worktree Delivery attempt maps to a child span of the Task trace.

- Span name: `spool.integration_attempt`
- Start time: attempt start
- End time: outcome recorded
- Attributes:
  - `spool.delivery.backend`
  - `spool.integration.outcome = "success" | "no_changes" | "work_change_failure" | "operational_failure"`
  - `spool.integration.strategy = "squash_merge"` for v1 successful changed work
  - `spool.integration.final_commit.recorded` as a boolean, not the commit by default
  - `spool.integration.cleaned_worktree`
  - `spool.integration.deleted_task_branch`

Merge conflicts, stale validation, dirty worktrees, repository locks, and cleanup failures should be span events with sanitized reason codes. Detailed Git output should stay local unless an Operator explicitly enables diagnostic payload export.

### Supervisor and Worker Loop events

Supervisor and Worker Loop observability should favor metrics and events over long-running spans.

Recommended events:

- `spool.supervisor.started`
- `spool.supervisor.exited`
- `spool.supervisor.lock_acquired`
- `spool.supervisor.lock_stale_removed`
- `spool.worker.claim_attempted`
- `spool.worker.claim_succeeded`
- `spool.worker.no_eligible_task`
- `spool.worker.run_timed_out`

Recommended attributes:

- `spool.queue.key`
- `spool.worker.concurrency`
- `spool.queue.concurrency_limit`
- `spool.supervisor.watch`
- `spool.supervisor.allow_overlap`

## Metrics mapping

A future exporter should derive OTel metrics from Spool records rather than write a separate metrics source of truth.

| Workflow Metric | OTel instrument | Suggested name | Dimensions |
| --- | --- | --- | --- |
| Task count by state | Observable gauge | `spool.tasks` | `spool.queue.key`, `spool.task.state`, `spool.priority` |
| Task lifecycle duration | Histogram | `spool.task.duration` | `spool.queue.key`, terminal state, priority |
| Time in Task State | Histogram | `spool.task.state.duration` | `spool.queue.key`, state |
| Agent Run duration | Histogram | `spool.agent_run.duration` | `spool.queue.key`, launcher kind, outcome |
| Agent Run outcomes | Counter | `spool.agent_run.outcomes` | `spool.queue.key`, launcher kind, outcome |
| Claim Lease expiries | Counter | `spool.claim_lease.expiries` | `spool.queue.key`, launcher kind |
| Retry holds | Observable gauge or counter | `spool.retry_holds` | `spool.queue.key`, reason code |
| Requirement status changes | Counter | `spool.requirement.status_changes` | requirement kind, from status, to status |
| Validation pass/fail counts | Counter | `spool.validation.results` | validation status, queue |
| Integration outcomes | Counter | `spool.integration.outcomes` | delivery backend, outcome |
| Human Review wait time | Histogram | `spool.human_review.duration` | queue, review decision when known |
| Queue throughput | Counter | `spool.queue.completed_tasks` | queue, terminal state |
| Transcript artifact size | Histogram or gauge | `spool.run_transcript.bytes` | launcher kind, queue |
| Token usage | Counter | `spool.launcher.tokens` | launcher kind, provider, model, token kind |
| Cost totals | Counter | `spool.launcher.cost` | launcher kind, provider, model, currency |
| Tool-call counts | Counter | `spool.launcher.tool_calls` | launcher kind, tool kind when safe |

Token and cost metrics should be emitted only when present in **Launcher Session Data**. They should not imply an external billing source of truth.

## Event mapping

**Audit Events** map naturally to OTel span events or log records. The future exporter should prefer sanitized event names and structured attributes over payload dumps.

Examples:

| Audit/Event source | OTel event/log name | Sanitized attributes |
| --- | --- | --- |
| Task created | `spool.task.created` | task identifier, queue, priority |
| State Transition requested/applied | `spool.task.state_transition` | from state, to state, actor kind |
| Claim created | `spool.agent_run.claimed` | agent run id, queue, launcher kind |
| Lease heartbeat gap/expiry | `spool.claim_lease.expired` | agent run id, seconds since heartbeat |
| Workpad Note updated | `spool.workpad.updated` | revision number or content length only |
| Requirement status set | `spool.requirement.status_changed` | kind, position, status |
| Child Task created | `spool.child_task.created` | parent identifier, child identifier, blocking boolean |
| Integration Outcome recorded | `spool.integration.outcome_recorded` | delivery backend, outcome |
| Cleanup run inspected/deleted artifacts | `spool.cleanup.runs` | dry-run/delete flag, counts, bytes |

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

If Spool later adds optional OTel export, the smallest safe path is:

1. Add a read-only exporter command or background task that derives telemetry from existing Spool data.
2. Start with metrics and sanitized logs/events before adding traces.
3. Use explicit Operator configuration for endpoint, protocol, headers, resource labels, and redaction policy.
4. Keep exporter state, retries, and failures separate from Spool workflow transactions.
5. Add deterministic tests using a fake OTel collector or in-memory exporter; keep real collector smoke tests opt-in.

No follow-up implementation Task is required by this mapping alone. Concrete follow-up Tasks should be created only when Spool is ready to implement optional export configuration, a read-only exporter, or additional sanitized metrics that current local telemetry cannot derive.

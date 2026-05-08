# Derive workflow metrics and store launcher session data

Tasker v1 will record enough event and run data to evaluate whether the local workflow is speeding up development: task cycle time by state, Ready/In Progress to Done time, Agent Run duration/outcome, retry and backoff counts, validation pass/fail counts, integration outcomes, Human Review wait time when used, and queue throughput. These Workflow Metrics will be derived from Audit Events, Agent Runs, Launcher Session Data, and Integration Outcomes rather than stored in a separate metrics database.

Each Agent Run may also store Launcher Session Data: normalized common fields such as launcher kind, session ID, model/provider, token/cost totals, tool-call counts, timestamps, and final status, plus raw launcher-specific artifacts or JSON. Pi v1 keeps the raw pi transcript/session data, while future launchers can attach their own detailed session data without changing the core metrics model.

Launcher Session Data stays local by default and is never uploaded or shared automatically. Raw transcripts live under the Tasker data directory; v1 should provide pruning/export commands and avoid deliberately logging API tokens, but it will not attempt broad automatic secret redaction.

# Agent Efficiency Strategy from Dogfood Sessions

## Purpose

This spike reviews Tasker dogfooding **Agent Runs** to identify where unattended **Worker Agents** lose time, duplicate work, or get blocked. The recommendations stay local-first: improve Tasker, the **Tasker Pi Extension**, prompts, task structure, and CLI observability without requiring external telemetry upload, web dashboards, or GitHub/PR-dependent workflow.

## Evidence reviewed

Representative local evidence sources:

- `tasker telemetry summary --queue TASKER` reported 99 Agent Runs, 36 duplicate or wasted runs across 24 Tasks, 28 post-Integrating Agent Runs, 9 failed/timed-out runs, and average completed-run duration of 226.5 seconds across 86 completed runs.
- `tasker telemetry lifecycle --queue TASKER` showed recent Ready-to-Done latency dominated either by agent execution (for example TASKER-63, TASKER-57, TASKER-58, TASKER-61, TASKER-64, TASKER-65) or by Integrating / Manual Dogfood Merge wait (for example TASKER-62 and TASKER-60).
- `tasker run show d1c053e3-2a31-4ae2-bde4-97b583748b0d` for TASKER-65 showed normalized efficiency data: 45 tool calls, a 52 MB Run Transcript, blocking extension UI detection, and optimization hints for excessive tool calls plus unexpected UI/questions.
- Older transcript-only artifacts under `.tasker/data/runs/<agent_run_id>/pi.jsonl` were sampled for TASKER-26 through TASKER-65. These runs often lack normalized metrics but include captured pi stdout sufficient to count `tool_execution_start` events and inspect representative tool patterns without embedding raw transcript bodies.
- Workpad Notes for TASKER-47 through TASKER-65 were sampled from Tasker state. Most final notes are concise implementation summaries with changed files and validation, while earlier plan/progress conventions vary.
- Failure examples from Task history and run records include unexpected question UI in TASKER-5 (`6ea4edf4-f8b3-409b-a26e-7329ca153f19`) and TASKER-7, local setup failures from dirty Managed Source Repository state in TASKER-52, TASKER-57, TASKER-63, and TASKER-64, an expired Claim Lease in TASKER-63 (`f1e429cf-f45c-41dc-a08e-8476a89ade78`), and operator cleanup after interrupted or duplicated supervisor runs in TASKER-19 and TASKER-24.

The investigation intentionally cites Task identifiers, Agent Run IDs, command summaries, and artifact paths rather than copying raw prompt or transcript bodies.

## Findings

### 1. Repeated context discovery is the most common small inefficiency

Worker Agents consistently read `CONTEXT.md` and `ROADMAP.md`, then run broad `find`/`rg` commands and inspect the same implementation files multiple times. This is appropriate for safety but creates repeated overhead:

- TASKER-65 (`d1c053e3-2a31-4ae2-bde4-97b583748b0d`) used 45 tool calls; sampled transcript events show 13 `read`, 25 `bash`, and 7 `edit` calls, including repeated reads of `crates/tasker-cli/src/monitor.rs` and `crates/tasker-db/src/lib.rs`.
- TASKER-58 (`2978eccc-1555-4fd5-8855-0e4eb8744057`) used 96 sampled tool calls and was the slowest recent completed Agent Run at 562 seconds; repeated reads included `crates/tasker-cli/src/worker.rs`, `crates/tasker-db/src/lib.rs`, and `crates/tasker-cli/src/main.rs`.
- TASKER-62's first run (`814869c4-7d57-45b4-884c-db3ad37e5556`) used 81 sampled tool calls and repeatedly inspected telemetry, CLI, database, and worker files.

This pattern is recurring, evidence-backed, and not merely anecdotal. It suggests the Worker Agent prompt and Tasker Pi Extension should help agents form a narrower initial file map.

### 2. Broad CLI usage fills gaps left by narrow Tasker Pi Extension tools

The unattended prompt correctly says Worker Agents should use Tasker Pi Extension tools for Tasker mutations, but the available workflow still pushes agents toward shell commands for context and observability:

- Agents repeatedly invoke `bin/tasker-local task show <TASK>`, `queue show`, `status`, and telemetry commands to confirm state, requirements, and safe database targeting.
- Agents use ad hoc SQLite or transcript parsing when answering cross-run questions because there is no first-class Tasker tool for recent Agent Run summaries, Workpad Note search, repeated failure patterns, or transcript-derived tool counts.
- Final requirement updates still often rely on CLI-compatible behavior in dogfood sessions when extension tool availability is unclear.

The CLI is valuable for Operators, but repeated shell inspection is inefficient for Worker Agents and weakens the product direction that the Tasker Pi Extension should expose narrow Tasker workflow tools.

### 3. Operational blockers have been a larger waste source than unclear code tasks

The biggest waste categories are not failed edits; they are workflow and local-state issues:

- Telemetry summary reports 36 duplicate/wasted Agent Runs across 24 Tasks and 28 post-Integrating Agent Runs.
- Dirty Managed Source Repository state caused immediate setup failures for TASKER-52, TASKER-57, TASKER-63, and TASKER-64.
- Unexpected or blocking UI caused failures or warnings, including TASKER-5 and TASKER-7 failed runs and TASKER-65's blocking extension UI detection despite successful completion.
- Manual Dogfood Merge / Integrating wait dominated some recent lifecycle latencies, especially TASKER-62 and TASKER-60.

These are recurring workflow issues. Recent fixes have already targeted them, but the evidence supports further lightweight guardrails and status surfacing.

### 4. Task briefs are generally sufficient, but task sizing and handoff conventions can improve

Recent Tasks have concrete Acceptance Criteria and Validation Items, and Workpad Notes usually capture changes and checks. However, agents still spend time reconstructing:

- Which docs/ADRs are relevant beyond `CONTEXT.md` and `ROADMAP.md`.
- Which code areas are expected to change.
- Whether a previous Agent Run on the same Task already completed useful work.
- Whether the Task Branch has been rebased or validated against current Main Branch.

Task Conflict Hints and Workpad Notes can reduce this if consistently populated and summarized. For larger implementation Tasks, Acceptance Criteria are necessary but not sufficient as an execution map.

### 5. Current telemetry is useful but still incomplete for diagnosis

Normalized Agent Run metrics are newly available and currently sparse. `tasker telemetry summary` reports only one run with normalized efficiency details at review time. Older runs require transcript parsing and cannot reliably expose:

- Unique files read versus repeated reads.
- Tool-call counts by command category.
- Time spent waiting on cargo, git, locks, or Tasker CLI builds.
- Token/context growth and cache behavior.
- Whether repeated Agent Runs were productive continuation, duplicate supervisor work, integration retry, or pure waste.

The gap is expected for a young local-first system, but it means future efficiency conclusions should separate normalized-metric evidence from transcript-derived estimates.

## Prioritized recommendations

| Priority | Area | Recommendation | Expected impact | Complexity | Evidence |
|---|---|---|---|---|---|
| P0 | Tools/extensions | Add a Tasker Pi Extension `get_task_context` style tool that returns Task brief, Acceptance Criteria, Validation Items, Workpad Note, Task Links, active/recent Agent Runs, current Local Worktree/Task Branch, and safe data-directory context in one response. | Fewer `task show`, `queue show`, `status`, and path-safety shell calls at run start. | Medium | Repeated `bin/tasker-local task show` and preflight shell calls across sampled transcripts. |
| P0 | Observability/telemetry | Extend normalized Agent Run metrics to count tool calls by tool name, tool errors, repeated reads of the same path, transcript byte size, blocking UI events, and shell-command categories. | Makes future diagnosis queryable without parsing huge transcripts. | Medium | TASKER-65 has 52 MB transcript and 45 tool calls; older runs require transcript parsing. |
| P0 | Workflow safety | Keep improving pre-claim local-state guards: surface dirty Managed Source Repository and operation-lock status before claims consume Agent Runs. | Reduces zero-second failed runs and duplicate retries. | Low/Medium | Dirty repo setup failures in TASKER-52, TASKER-57, TASKER-63, TASKER-64. |
| P1 | Prompts/context | Update the Worker Agent Role Prompt to require a short context plan: list relevant files/ADRs once, avoid rereading unchanged files, prefer `rg` narrowing before broad reads, and summarize local evidence in the Workpad Note. | Reduces repeated reads while preserving safety. | Low | Repeated reads in TASKER-58, TASKER-62, TASKER-65, TASKER-60, TASKER-33. |
| P1 | Task sizing/briefs | Add a bootstrap/delegation convention for `likely files`, `relevant ADRs`, and `expected validation commands` in the Task Brief or structured Task Conflict Hints. | Helps agents start narrow and choose checks earlier. | Low | Agents repeatedly infer file maps with broad `find`/`rg`; Task Conflict Hints are currently often empty. |
| P1 | Tools/extensions | Add Tasker Pi Extension tools for requirement status updates, Workpad updates, Task Link creation, and transition requests that include current Agent Run attribution and validation base commit handling. | Reduces broad CLI mutation usage and enforces Worker Agent boundaries. | Medium | Worker prompt mandates extension tools; dogfood often falls back to CLI-compatible workflows. |
| P1 | Observability/status | Add `tasker run summary --queue TASKER --recent N` or equivalent API/tool output showing recent failures, duplicate runs, blocking UI, and per-task run history. | Faster diagnosis before spawning more work. | Low/Medium | Telemetry summary is helpful but cross-run details require multiple commands or SQL. |
| P2 | Workflow | Add a post-Agent-Run handoff template in Workpad Notes: summary, files changed, validation, known risks, base commit, follow-up Task candidates. | Makes retry/rework continuation cheaper. | Low | Workpad Notes are concise but vary in structure and do not always expose continuation state uniformly. |
| P2 | Telemetry | Capture elapsed time by phase where possible: setup, initial context loading, editing, validation, Tasker mutation, integration wait. | Separates agent reasoning cost from cargo/git/lock wait. | Medium/High | Lifecycle summary distinguishes high-level phases but not within-run causes. |
| P2 | Monitor/status | Highlight recovered failures separately from active attention, with links to the later successful Agent Run and Task state. | Prevents operators/agents from chasing stale failures. | Low | TASKER-65 implemented part of this after monitor surfaced recovered failures. |
| P3 | Larger post-Dogfooding work | Explore local-only aggregate efficiency reports over Audit Events, Agent Runs, Integration Outcomes, and launcher artifacts. | Helps long-term tuning without external telemetry. | Medium/High | Current summaries are useful but still young and sparse. |

## Suggested follow-up Task candidates

1. **Add Tasker Pi Extension task-context bundle**
   - Acceptance idea: Worker Agent can fetch Task brief, structured requirements, Workpad Note, Task Links, Local Worktree metadata, recent Agent Runs, and active data/config context in one extension call.
   - Validation idea: contract test against a test Tasker Service verifies no broad CLI calls are required for initial task context.

2. **Persist per-tool efficiency metrics from Run Transcripts**
   - Acceptance idea: `agent_run_metrics` stores tool-call counts by tool name, repeated read count, tool error count, blocking UI count, and transcript byte size/event count.
   - Validation idea: fake pi transcript fixture produces deterministic metric rows and telemetry summary output.

3. **Add recent Agent Run diagnostic summary**
   - Acceptance idea: CLI/API summarizes recent Agent Runs by Task with outcome, failure reason, duration, duplicate/waste classification, blocking UI, and transcript path.
   - Validation idea: temp SQLite test covers completed, failed, expired, and recovered failure sequences.

4. **Document and prompt a Workpad Note handoff template**
   - Acceptance idea: Worker Agent Role Prompt and docs define a concise Workpad Note shape for plan, changes, validation, risks, follow-ups, and base commit.
   - Validation idea: documentation-only check confirms terms match `CONTEXT.md` and no Workpad Markdown is described as authoritative gate state.

5. **Populate Task Conflict Hints during bootstrap/delegation**
   - Acceptance idea: bootstrap Task files can include likely paths/areas and `task show` surfaces them prominently for Worker Agents.
   - Validation idea: bootstrap parsing test and task-show snapshot verify hints remain advisory and do not block scheduling.

6. **Pre-claim Managed Source Repository cleanliness check in supervisor output**
   - Acceptance idea: supervisor reports dirty repo / operation lock before claiming when possible, avoiding zero-duration failed Agent Runs.
   - Validation idea: temp Git repository test covers clean, dirty, and stale-lock conditions.

## Token and context telemetry spike update

Local pi Run Transcripts do expose exact provider-style usage signals in JSONL stdout events. Recent sampled transcripts include `message.usage.input`, `message.usage.output`, `message.usage.totalTokens`, `message.usage.cacheRead`, and `message.usage.cacheWrite`, with the same fields repeated in streaming `assistantMessageEvent.partial.usage` payloads. Tasker now parses these fields, stores normalized input/output/total/cache-read/cache-write tokens, and treats the maximum observed `totalTokens` as the best available per-run context pressure signal. Raw prompt, transcript, and tool argument bodies remain in local Run Transcript artifacts only; human and JSON telemetry output expose numeric summaries rather than raw bodies.

Exact metrics currently available from local artifacts:

- Input tokens, output tokens, total tokens, cache-read tokens, and cache-write tokens from pi usage events.
- Maximum observed context pressure from `totalTokens` or explicit context-token fields when present.
- Tool-call count, tool-error count, repeated failed tool attempts, assistant/user turn counts, blocking UI signals, Run Transcript byte size, and JSONL event count.
- Per-tool call counts for safe tool names, repeated file-read counts by path within one Agent Run, repeated Tasker context fetch counts, and shell command category counts for Tasker CLI, cargo, git, search, and other commands.

Metrics still unavailable or approximate:

- Provider context-window size / maximum possible context tokens is not present in sampled pi artifacts.
- Shell output volume remains approximate through Run Transcript byte size and safe command categories; Tasker does not persist raw command output or tool argument payloads as metrics.
- Existing Agent Runs may need a metrics refresh path before old rows show newly parsed token/cache/tool-efficiency fields.

Near-term dogfooding can continue, but broadening dogfooding should use the first-class proxy metrics to identify repeated file reads, repeated Tasker context fetches, broad shell/CLI inspection, duplicate Agent Runs, and local-state setup failures. Exact token telemetry and safe per-tool summaries now make these hypotheses testable on new Agent Runs.

## Dogfood efficiency budget defaults

Tasker surfaces local dogfood efficiency budget warnings in `tasker telemetry summary` and `tasker run show` so regressions are visible without inspecting raw Run Transcripts. These thresholds are dogfooding tuning defaults, not permanent product policy: operators should adjust them as Tasker improves and new baselines emerge.

Current defaults flag warning/severe overruns at 150/250 tool calls, 80k/100k total tokens, 80k/100k max context tokens, 100 MiB/200 MiB Run Transcript byte size, 1/10 repeated file reads, 1/5 repeated Tasker context fetches, and 1h/2h run duration. Missing token and context metrics are reported as unknown rather than passing or failing as zero, and token budget output labels exact token metrics separately from proxy-only runs.

## Guidance for near-term dogfooding

- Prefer small implementation Tasks with explicit relevant ADRs, likely files, and validation commands.
- Treat Workpad Notes as handoff summaries, not authoritative requirement state; keep structured Acceptance Criteria and Validation Items current.
- Continue using `tasker telemetry summary`, `tasker telemetry lifecycle`, `tasker run show`, and `.tasker/data/runs/<id>/pi.jsonl` as local evidence sources until extension/API equivalents exist.
- Prioritize reducing duplicate Agent Runs and local-state setup failures before optimizing model-level behavior; the current evidence shows workflow waste is more measurable than token/context waste.
- Keep all telemetry local by default and derive Workflow Metrics from Audit Events, Agent Runs, Launcher Session Data, and Integration Outcomes.

## Gaps and caveats

- Only recent runs have normalized Agent Run efficiency metrics; older evidence is transcript-derived and may undercount or overcount if event formats changed.
- Transcript files can be very large and contain prompt/tool content, so strategy documents should cite paths and summaries rather than embedding raw bodies.
- Earlier Tasker metrics did not parse pi's camelCase usage fields, so older database rows may still show unknown token/cache values until their metrics are refreshed or new Agent Runs finish.
- Some duplicate Agent Runs were caused by known dogfooding transitions and may not recur after recent supervisor, monitor, and integration fixes.

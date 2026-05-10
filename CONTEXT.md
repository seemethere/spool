# Tasker

Tasker provides a minimal local-first task backend for Symphony-style agent orchestration without making Linear or GitHub a required dependency. Tasker should become useful enough to dogfood on its own development as quickly as possible.

## Language

**Task Backend**:
A local-first Symphony-compatible task source and sink that owns the task records and collaboration data needed by agent runs.
_Avoid_: Linear clone, issue tracker clone, project management system, pull-request workflow manager

**Tasker API**:
The first-class contract Symphony uses to read and update Tasker-owned task data.
_Avoid_: Linear-compatible GraphQL facade, fake Linear API

**Tasker Service**:
The HTTP service that owns **Tasks**, **Task Queues**, **Task States**, **Agent Runs**, and delivery records.
_Avoid_: Agent runner

**Symphony Adapter**:
The thin runner-side integration layer that uses the **Tasker API** to run the local Tasker workflow end-to-end.
_Avoid_: Linear shim, API-only proof

**Worker Loop**:
The `tasker work` process that claims eligible **Tasks** from a **Task Queue** and runs them through the **Symphony Adapter**.
_Avoid_: Hidden worker inside Tasker Service

**Dogfooding Readiness**:
The early milestone where Tasker can run useful Tasks against its own repository.
_Avoid_: Waiting for full polish before self-use

**Pre-Dogfooding Development Loop**:
The temporary manual human/agent workflow used to build Tasker until **Dogfooding Cutover**.
_Avoid_: Permanent delivery workflow, ad hoc forever process

**Implementation Slice**:
A small reviewable unit of pre-dogfooding work that advances one roadmap milestone and can be tested or documented independently.
_Avoid_: Whole-milestone batch, vague chat request, giant branch

**Slice Acceptance Checks**:
The explicit deterministic tests, formatting, linting, and documentation expectations that prove an **Implementation Slice** is complete.
_Avoid_: Vibes-based done, hidden reviewer expectations

**Approved Slice Sequence**:
A human-approved run of multiple low-risk **Implementation Slices** that the agent may implement and commit one-by-one without pausing after each slice.
_Avoid_: Unbounded autonomous development, mixed mega-commit

**Subagent Review Loop**:
The pre-dogfooding practice of using advisory subagents to review diffs and escalate decision conflicts before committing an **Implementation Slice**.
_Avoid_: Parallel implementation, replacing human approval, Tasker Review Session

**Oracle Escalation**:
A **Subagent Review Loop** step where an oracle subagent resolves documented decision conflicts or stop-condition ambiguity before implementation continues.
_Avoid_: Guessing through architecture/security/domain contradictions

**Dogfooding Cutover**:
The point where Tasker development work starts being managed as real Tasker **Tasks**.
_Avoid_: Full v1 completion, polished launch

**Bootstrap Task Creation**:
A temporary dogfooding shortcut for creating a **Task** from a Markdown file with YAML front matter before the full **Delegation Session** is ready.
_Avoid_: Permanent manual intake model

**Manual Dogfood Merge**:
A temporary dogfooding practice where the human inspects and merges a completed **Local Worktree** before automatic **Integrating** is implemented.
_Avoid_: Permanent delivery model

**Worker Concurrency**:
The number of **Agent Runs** a **Worker Loop** may execute at the same time.
_Avoid_: Unbounded local parallelism

**Task Queue**:
An operator-managed named grouping of Tasks that a Symphony workflow polls for work.
_Avoid_: Project, Linear project, team, milestone, cycle, agent-created queue

**Operator**:
The deployment owner who manages Tasker infrastructure boundaries such as **Task Queues** and repair actions.
_Avoid_: Task author

**Actor**:
The attributed source of a Tasker mutation, identified by stable ID, kind, and display name, and supplied explicitly by the caller.
_Avoid_: Anonymous mutation, bearer token identity

**Worker Agent**:
An **Actor** that executes an **Agent Run** for a **Task**.
_Avoid_: Delegating Agent, Review Agent

**Agent Launcher**:
A pluggable runner-side integration that starts and communicates with a coding agent for an **Agent Run**.
_Avoid_: Tasker-owned agent protocol

**Pi Launcher**:
The v1 **Agent Launcher** that runs Worker Agents through pi RPC mode.
_Avoid_: Codex-only launcher, one-shot print mode

**Pi RPC Session**:
A `pi --mode rpc` process controlled by the **Pi Launcher** over JSONL stdin/stdout.
_Avoid_: TypeScript SDK dependency in the Rust adapter

**Tasker Pi Extension**:
A pi extension that exposes narrow Tasker workflow tools to Worker Agents.
_Avoid_: Shelling out to broad CLI commands for core workflow updates

**Run Transcript**:
The saved pi session output and metadata for one **Agent Run**.
_Avoid_: Hidden continuation state

**Launcher Session Data**:
Normalized and raw session metadata captured from an **Agent Launcher** for an **Agent Run** and stored locally by default.
_Avoid_: Pi-only metrics schema, automatic upload

**Workflow Metric**:
A measurement derived from **Audit Events**, **Agent Runs**, and **Integration Outcomes** to evaluate local workflow speed and reliability.
_Avoid_: Separate metrics source of truth

**Interactive Agent Session**:
A pi-backed session where a human is intentionally present, such as a **Delegation Session** or **Review Session**.
_Avoid_: Unattended worker prompt

**Unattended Worker Session**:
A pi-backed **Agent Run** started by a **Worker Loop** without a human waiting to answer questions.
_Avoid_: Hidden human dependency

**Task Queue Key**:
A short stable code used as the prefix for human-readable **Task Identifiers**.
_Avoid_: Project slug

**Queue Concurrency Limit**:
An optional cap on the number of active **Agent Runs** for a **Task Queue**.
_Avoid_: Global Symphony limit

**Task Identifier**:
A stable human-readable key generated as `<TASK_QUEUE_KEY>-<sequence>`.
_Avoid_: UUID-only identifier, Linear issue key

**Delegating Agent**:
An agent that creates or updates a **Task** so another agent can perform the work.
_Avoid_: Human task author

**Delegation Session**:
A local `tasker delegate` interaction where a **Delegating Agent** turns human intent into a **Root Task**.
_Avoid_: Manual task form

**Delegation Interview**:
A one-question-at-a-time clarification flow, modeled on grill-with-docs, used during a **Delegation Session** to clarify a **Task** without editing repository docs by default.
_Avoid_: Bulk intake form, vague task dump, hidden source edit

**Review Agent**:
An agent that translates external human approval or feedback into Tasker **State Transitions**.
_Avoid_: Human Tasker operator

**Review Session**:
A local `tasker review` interaction where a **Review Agent** prepares a **Review Packet** and records a human **Review Decision**.
_Avoid_: GitHub review dependency, Tasker web UI

**Review Decision**:
A recorded approval or feedback outcome that moves a **Task** out of **Human Review**.
_Avoid_: Unrecorded review signal

**Review Packet**:
A concise artifact prepared by a **Review Agent** to help a human decide whether a **Task** should be reworked or integrated.
_Avoid_: Tasker review UI

**Review Policy**:
A queue or task rule that decides whether completed work requires **Human Review** or may proceed through **Agent-Gated Integration**.
_Avoid_: Mandatory human bottleneck, implicit self-approval

**Agent-Gated Integration**:
A review policy where a **Worker Agent** may move work to **Integrating** after structured gates pass, without a human review step.
_Avoid_: Ungated auto-merge

**Task**:
A unit of work that a Symphony agent can claim, execute, update, and hand off.
_Avoid_: Linear issue, ticket

**Task Brief**:
The Markdown narrative that explains the context and requested outcome of a **Task**.
_Avoid_: Issue description, body blob

**Priority**:
A Task ordering label: **urgent**, **high**, **normal**, or **low**.
_Avoid_: Arbitrary score, manual rank

**Acceptance Criterion**:
A required outcome that must be true before a **Task** can be handed off or completed.
_Avoid_: Nice-to-have, vague requirement

**Criterion Status**:
The recorded completion state of an **Acceptance Criterion**: pending, satisfied, or waived.
_Avoid_: Markdown-only checkbox

**Validation Item**:
A required proof step that demonstrates the **Acceptance Criteria** have been met.
_Avoid_: Optional test note

**Validation Status**:
The recorded proof state of a **Validation Item**: pending, passed, failed, or waived.
_Avoid_: Markdown-only test note

**Waiver**:
An explicit reason for handing off or completing work without satisfying an **Acceptance Criterion** or passing a **Validation Item**.
_Avoid_: Silent exception

**Task Tag**:
A lightweight metadata label used to categorize **Tasks** without changing scheduling behavior.
_Avoid_: Linear label, scheduling rule

**Task Link**:
A typed reference attached to a **Task**, such as a worktree path, branch, diff, log, chat thread, or media artifact.
_Avoid_: Built-in GitHub integration, pull-request-only model

**Primary Handoff Link**:
The main **Task Link** a reviewer or finishing agent should inspect for handoff or delivery.
_Avoid_: Primary Pull Request, GitHub PR dependency, multiple competing handoff links

**Delivery Backend**:
A pluggable strategy for preparing, reviewing, and integrating completed **Task** work.
_Avoid_: Task Backend, hard-coded PR workflow

**Delivery Adapter**:
The Symphony-side component that performs the filesystem or external-system operations for a **Delivery Backend**.
_Avoid_: Tasker-run Git command

**Delivery Record**:
Tasker's record of delivery-related facts and outcomes for a **Task**.
_Avoid_: Execution script

**Local Worktree Delivery**:
The v1 **Delivery Backend** where work happens in per-Task Git worktrees and is integrated locally.
_Avoid_: GitHub delivery, pull-request delivery

**Managed Source Repository**:
The local Git repository treated as the source of truth for **Local Worktree Delivery** and owned by the Tasker/Symphony workflow.
_Avoid_: Casual working copy, unrelated manual worktree

**Worktree Root**:
The local directory where per-Task **Local Worktrees** are created.
_Avoid_: Workspace root ambiguity

**Task Branch**:
The Git branch used for a **Task**'s **Local Worktree**.
_Avoid_: Pull request branch

**Task Commit**:
A local commit on a **Task Branch** containing work for a **Task**.
_Avoid_: Uncommitted worktree change

**Final Commit**:
The Main Branch commit that contains a completed **Task**'s delivered work.
_Avoid_: Worktree as durable record

**Validated Base Commit**:
The Main Branch commit against which a **Task Branch** was last validated.
_Avoid_: Stale validation

**Done Worktree Retention**:
A queue option that keeps completed **Local Worktrees** for debugging instead of cleaning them up after delivery.
_Avoid_: Permanent worktree clutter

**Branch Template**:
The pattern used to derive a **Task Branch** from a **Task Identifier**.
_Avoid_: Ad-hoc branch name

**Role Prompt**:
Instructions for a Delegating Agent, Worker Agent, or Review Agent.
_Avoid_: Runtime configuration

**Prompt Override**:
A repo-owned replacement for a built-in **Role Prompt** under `.tasker/prompts/`.
_Avoid_: Hidden global prompt

**Local Worktree**:
A per-Task Git worktree where a **Worker Agent** performs implementation work.
_Avoid_: Pull request branch as the primary workspace

**Main Branch**:
The local integration branch that completed work is merged into.
_Avoid_: Remote PR target

**Local Merge**:
The act of integrating completed work from a **Local Worktree** into the **Main Branch**.
_Avoid_: GitHub merge, PR merge

**Squash Merge**:
The default **Local Merge** strategy that turns a **Task Branch** into one **Final Commit** on the **Main Branch**.
_Avoid_: Multi-commit main history by default

**Integration Outcome**:
The recorded result of a **Delivery Adapter** attempting to deliver approved work, including successful merge, no changes, work-change failure, or operational failure.
_Avoid_: Unrecorded merge result

**No-Change Integration**:
An **Integration Outcome** showing a completed **Task** had no repository changes to merge.
_Avoid_: Skipping delivery finalization

**Work-Change Delivery Failure**:
An **Integration Outcome** showing the Task work must change before delivery can succeed.
_Avoid_: Transient retry

**Operational Delivery Failure**:
An **Integration Outcome** showing delivery failed because of local tooling, locks, interruption, or infrastructure rather than Task content.
_Avoid_: Rework-required failure

**Delegated Task**:
A **Task** created by a **Delegating Agent** for agent execution.
_Avoid_: Manually entered task

**Root Task**:
A **Delegated Task** created from an out-of-band human request.
_Avoid_: Manually created task

**Child Task**:
A **Delegated Task** created while an agent is working another **Task**.
_Avoid_: Sub-issue, subtask

**Blocking Task**:
A **Task** that must reach **Done** before another **Task** can be completed.
_Avoid_: Automatic child dependency, canceled dependency

**Blocked Task**:
A **Task** that cannot complete because one or more **Blocking Tasks** are not **Done**.
_Avoid_: Blocked state

**Follow-up Task**:
A non-blocking **Child Task** that records discovered work without expanding the parent **Task** scope.
_Avoid_: Nice-to-have inside current task

**Task State**:
One of Tasker's fixed v1 lifecycle labels: **Backlog**, **Ready**, **In Progress**, **Human Review**, **Rework**, **Integrating**, **Done**, or **Canceled**.
_Avoid_: Linear status, issue status, custom workflow state, Todo

**Backlog**:
A **Task State** for work that exists but is not eligible for agent pickup.
_Avoid_: Icebox

**Ready**:
A **Task State** for work that is queued for agent pickup and has enough requirements for autonomous execution.
_Avoid_: Todo

**In Progress**:
A **Task State** for work in its active execution phase, whether or not a live agent process is currently attached.
_Avoid_: Started, running process

**Agent Run**:
A Tasker-persisted claim lease for a live agent execution attempt of a **Task**.
_Avoid_: Task State, In Progress, agent subprocess

**Claim Lease**:
A time-bounded reservation that prevents more than one worker from picking the same **Task** at the same time.
_Avoid_: In-memory claim only

**Lease Heartbeat**:
A periodic signal from a **Worker Loop** that keeps an **Agent Run**'s **Claim Lease** active.
_Avoid_: Agent output as liveness

**Agent Run Outcome**:
The execution result recorded when an **Agent Run** ends: completed, failed, canceled, or expired.
_Avoid_: Task State

**Expired Agent Run**:
An **Agent Run** whose **Claim Lease** ended without a normal finish signal.
_Avoid_: Task rollback

**Retry Hold**:
A temporary per-Task delay that prevents immediate re-claim after a failed or expired **Agent Run**.
_Avoid_: Tight retry loop

**Human Review**:
A **Task State** for work waiting on human approval or feedback.
_Avoid_: Review

**Rework**:
A **Task State** for work that an agent should revise after feedback or integration failure.
_Avoid_: Changes requested, default reset

**Reset Rework**:
An explicit decision to discard a **Task**'s current **Local Worktree** and restart from the **Main Branch**.
_Avoid_: Automatic rework reset

**Integrating**:
A **Task State** for approved work being delivered by the **Task Queue**'s configured **Delivery Backend**.
_Avoid_: Merging, GitHub merge, PR merge, Landing

**Done**:
A terminal **Task State** for successfully completed work.
_Avoid_: Closed

**Canceled**:
A terminal **Task State** for abandoned work whose active run and unintegrated local delivery artifacts should be cleaned up by default.
_Avoid_: Cancelled, Duplicate

**State Transition**:
An allowed movement from one **Task State** to another.
_Avoid_: Status update, free-form state edit

**Repair Override**:
An operator-only state or requirement change that bypasses normal workflow rules to fix bad data or operational mistakes.
_Avoid_: Admin shortcut, agent tool

**Audit Event**:
An append-only history record of a Tasker domain mutation.
_Avoid_: Event-sourced state source, task-only event

**Workpad Note**:
The single persistent agent-authored narrative note on a **Task** used for plan, evidence, and handoff communication.
_Avoid_: Scratchpad comment, progress comment, ordinary comment stream, authoritative checkbox state

**Workpad Revision**:
A saved historical version of a **Workpad Note**.
_Avoid_: Separate progress comment

## Relationships

- A **Task Backend** contains many **Task Queues**.
- An **Operator** manages **Task Queues** through `tasker queue create`, `tasker queue update`, and `tasker queue show`.
- Creating a **Task Queue** for **Local Worktree Delivery** warns that the repository becomes a **Managed Source Repository**.
- A **Task Queue** has exactly one **Task Queue Key**.
- A **Task Queue** may have one **Queue Concurrency Limit**.
- A **Task Queue** has exactly one configured **Delivery Backend**.
- A **Task Queue** configured for **Local Worktree Delivery** has a **Managed Source Repository**, **Main Branch**, **Worktree Root**, and **Branch Template**.
- A **Task Queue** contains many **Tasks**.
- A **Task** has exactly one **Task Identifier**.
- A **Task** has exactly one **Task Brief**.
- A **Task** has exactly one **Priority**, defaulting to **normal**.
- **Priority** may be changed by a **Delegating Agent**, **Review Agent**, **Operator**, or explicit system policy.
- A **Worker Agent** does not directly reprioritize work during execution.
- Eligible **Tasks** are claimed by **Priority**, then creation time, then **Task Identifier**.
- A **Task** has zero or more **Acceptance Criteria**.
- Each **Acceptance Criterion** has one **Criterion Status**.
- A **Task** has zero or more **Validation Items**.
- Each **Validation Item** has one **Validation Status**.
- A **Waiver** requires an explicit reason.
- A **Waiver** may be created by a **Review Agent** or **Operator**, not by a **Worker Agent**.
- A **Worker Agent** may add or clarify **Acceptance Criteria** and **Validation Items** during **In Progress** or **Rework**.
- Removing requirements or waiving them requires a **Review Agent** or **Operator**.
- Substantive edits to **Acceptance Criteria** or **Validation Items** reset their status to pending.
- A **Task** may have many **Task Tags**.
- **Task Tags** do not affect v1 scheduling or eligibility.
- A **Task** may have many **Task Links**.
- A **Delivery Adapter** performs **Delivery Backend** operations outside Tasker.
- Tasker stores **Delivery Records** and **Integration Outcomes**.
- **Local Worktree Delivery** is the only v1 **Delivery Backend**.
- Individual **Tasks** do not choose their own **Delivery Backend** in v1.
- A **Task** may have one **Local Worktree**.
- A **Task** may have one **Task Branch**.
- A **Task Branch** may have many **Task Commits**.
- A completed **Task** may have one **Final Commit**.
- A **Task** may have one **Validated Base Commit**.
- A **Task** has at most one **Primary Handoff Link**.
- A **Local Merge** integrates a **Local Worktree** into the **Main Branch**.
- **Squash Merge** is the default **Local Merge** strategy.
- A **Final Commit** uses the **Managed Source Repository**'s configured Git identity.
- A **Final Commit** includes Tasker metadata such as **Task Identifier**, title, and optionally run ID in its commit message.
- **Integrating** requires a clean **Local Worktree** with changes committed as **Task Commits** on the **Task Branch**.
- A **Local Merge** requires the **Task Branch** to include the current **Main Branch** or the **Validated Base Commit** to equal the current **Main Branch**.
- If the **Validated Base Commit** is stale or the **Local Worktree** has uncommitted changes, the **Integration Outcome** is a **Work-Change Delivery Failure**.
- **No-Change Integration** moves a **Task** from **Integrating** to **Done** without a merge.
- After a successful **Local Merge**, the **Delivery Adapter** removes the **Local Worktree** and deletes the **Task Branch** unless **Done Worktree Retention** is enabled.
- A **Work-Change Delivery Failure** moves a **Task** from **Integrating** to **Rework**.
- **Rework** continues from the existing **Local Worktree** and **Task Branch** by default.
- **Reset Rework** discards the current **Local Worktree** and restarts from the **Main Branch**.
- An **Operational Delivery Failure** leaves a **Task** in **Integrating** for retry.
- Tasker does not run arbitrary Git commands for **Local Worktree Delivery**.
- Configuring **Local Worktree Delivery** opts the **Operator** into Tasker/Symphony mutating the **Managed Source Repository**.
- The **Managed Source Repository** may contain **Prompt Overrides** at `.tasker/prompts/delegate.md`, `.tasker/prompts/worker.md`, and `.tasker/prompts/review.md`.
- Unexpected uncommitted changes in the **Managed Source Repository** are an **Operational Delivery Failure**.
- Every Tasker mutation is attributed to an **Actor** or the system.
- Authentication identifies the API client; **Actor** identifies the source of the domain change.
- Every Tasker domain mutation produces an **Audit Event**.
- Current Tasker state is read from current records, not reconstructed from **Audit Events**.
- `tasker delegate` starts a **Delegation Session**.
- `tasker delegate --refine` starts a **Delegation Session** for an existing **Backlog** **Task**.
- A **Delegation Session** uses a **Delegation Interview** and the **Tasker Pi Extension**.
- A **Delegation Interview** may read repository context docs but does not edit them by default.
- Repository doc changes discovered during delegation become **Acceptance Criteria** or **Child Tasks**.
- A **Delegating Agent** creates **Delegated Tasks**.
- A **Worker Agent** executes **Agent Runs**.
- `tasker review` starts a **Review Session**.
- A **Review Agent** may prepare a **Review Packet**.
- A **Review Agent** records **Review Decisions**.
- A **Review Decision** moves a **Task** from **Human Review** to **Rework** or **Integrating**.
- Local-first queues default to **Agent-Gated Integration**.
- A **Worker Agent** may move **In Progress** or **Rework** work to **Integrating** when structured gates pass and **Review Policy** does not require **Human Review**.
- When the same **Agent Run** still owns the **Claim Lease**, it may perform **Integrating** immediately after the **State Transition**.
- **Human Review** requires a human decision when **Review Policy** requires review or an agent explicitly requests review.
- A **Root Task** has no parent **Task**.
- A **Root Task** starts in **Backlog** by default unless a **Delegating Agent** explicitly requests **Ready**.
- A **Child Task** has exactly one parent **Task** in the same **Task Queue**.
- A **Child Task** may be a **Blocking Task** for its parent.
- **Blocking Task** relationships are same-queue only in v1.
- A **Blocked Task** has at least one **Blocking Task** that is not **Done**.
- A **Blocked Task** is excluded from normal agent pickup.
- A **Blocked Task** cannot transition to **Human Review**, **Integrating**, or **Done** without resolving its **Blocking Tasks** or using a **Repair Override**.
- A **Task** cannot transition to **Human Review**, **Integrating**, or **Done** unless all **Acceptance Criteria** are satisfied or waived and all **Validation Items** are passed or waived.
- Structured Tasker fields, not **Workpad Note** Markdown, are authoritative for gates and scheduling.
- A **Follow-up Task** does not block its parent.
- A **Follow-up Task** starts in **Backlog** by default.
- A **Task** belongs to exactly one **Task Queue**.
- A **Task** has exactly one current **Task State**.
- **Ready**, **In Progress**, **Rework**, and **Integrating** are agent-eligible **Task States**.
- A **Task** must have at least one **Acceptance Criterion** and one **Validation Item** before entering **Ready**, unless using a **Repair Override**.
- **Backlog** and **Human Review** are not agent-eligible **Task States**.
- **Done** and **Canceled** are terminal **Task States**.
- Moving a **Task** to **Canceled** cancels active **Agent Runs** and cleans unintegrated **Local Worktree**/**Task Branch** artifacts by default.
- Normal clients change **Task State** only through allowed **State Transitions**.
- A **Repair Override** may bypass normal **State Transitions**.
- A **Repair Override** requires an **Operator** actor and explicit reason.
- Worker and Delegating Agents cannot use **Repair Overrides**.
- A **Task** has at most one active **Workpad Note**.
- A **Workpad Note** may have many **Workpad Revisions**.
- Symphony reads from and writes to the **Task Backend** through the **Tasker API**.
- `tasker init` creates XDG-style local config and data directories.
- Tasker config defaults to `~/.config/tasker/config.toml`.
- Tasker data defaults to `~/.local/share/tasker/`, with SQLite at `tasker.db` and run transcripts under `runs/<run_id>/`.
- When a repository-local `.tasker/config.toml` is present but not the resolved active Tasker config, unsafe mutating CLI commands refuse to run unless the operator explicitly selects a config or data/database override.
- In that inactive project-config case, read-only CLI commands warn with the active config and database paths so operators can diagnose wrong-database mistakes.
- `tasker serve` starts the **Tasker Service**.
- `tasker work` starts a **Worker Loop**.
- A **Worker Loop** has default **Worker Concurrency** of 1.
- `tasker work --concurrency N` raises **Worker Concurrency** up to the **Queue Concurrency Limit**.
- `tasker work --once` claims and runs at most one **Task**.
- `tasker work --max-run-seconds N` bounds one launcher execution and fails the **Agent Run** if the **Pi Launcher** does not emit `agent_end` before the duration elapses.
- `tasker status` shows queue counts, running work, and retry holds.
- `tasker task show` shows full **Task** state.
- `tasker task retry` is an **Operator** recovery command that clears a **Retry Hold** and moves a resolved failed, canceled, or stuck **Task** back to **Ready** without changing normal completion gates.
- `tasker run show` shows **Agent Run**, **Run Transcript**, and **Launcher Session Data** metadata.
- `tasker run fail` is an **Operator** recovery command that fails an active **Agent Run** with an explicit reason and records a **Retry Hold**.
- Dogfooding API covers health/version, queue create/show/list, bootstrap task create, task show, claim-next, heartbeat, finish-run, Workpad Note update, criterion/validation status update, child task creation, state transition request, local worktree/delivery metadata, status summary, run show, and operator recovery for failed or stuck work.
- Search, bulk edits, review sessions, pruning, metrics export, and token admin APIs are deferred until after **Dogfooding Readiness**.
- Dogfooding persistence includes queues, tasks, acceptance criteria, validation items, workpad notes/revisions, task links, task relationships, agent runs/heartbeats, delivery records, launcher session data, audit events, and API tokens.
- **Dogfooding Readiness** comes before full v1 polish.
- **Pre-Dogfooding Development Loop** is used before **Dogfooding Cutover**.
- **Pre-Dogfooding Development Loop** operates on one **Implementation Slice** at a time.
- During the **Pre-Dogfooding Development Loop**, the agent proposes the next **Implementation Slice** and the human approves or redirects it.
- An **Implementation Slice** advances exactly one current roadmap milestone.
- Each **Implementation Slice** has **Slice Acceptance Checks** before implementation begins.
- **Slice Acceptance Checks** include relevant targeted tests plus formatting and linting when configured and reasonably cheap to run.
- A **Pre-Dogfooding Development Loop** inspects docs/code, proposes an **Implementation Slice**, confirms **Slice Acceptance Checks**, implements, runs deterministic checks, updates docs when domain or behavior changes, and summarizes the result.
- Documentation changes are part of an **Implementation Slice** when domain language, workflow behavior, persistence meaning, delivery behavior, launcher behavior, or milestone sequencing changes.
- Pre-dogfooding loop rules and cutover criteria are recorded in `docs/PRE_DOGFOODING_LOOP.md` until Tasker can track real development **Tasks**.
- A completed **Pre-Dogfooding Development Loop** normally ends with unstaged or staged working tree changes, a concise summary, and a recommended Conventional Commit message; the human decides whether the agent commits.
- In an **Approved Slice Sequence**, the agent may implement and commit multiple approved low-risk **Implementation Slices** in order, stopping when scope, architecture, security, persistence semantics, task lifecycle, delivery behavior, launcher behavior, or unresolved check failures require human input.
- A **Subagent Review Loop** runs before committing implementation slices during pre-dogfooding work.
- **Subagent Review Loop** reviewers are advisory development helpers and are distinct from Tasker's domain **Review Agent**.
- **Oracle Escalation** is used when a reviewer finding, implementation discovery, or user instruction exposes a conflict with documented architecture, security, persistence semantics, task lifecycle, delivery behavior, launcher behavior, or domain language.
- **Dogfooding Cutover** occurs when Tasker development work starts being created and tracked as real Tasker **Tasks**.
- The first **Dogfooding Cutover** target is after roadmap Milestone 2, when **Bootstrap Task Creation**, **Task Queues**, task show/status, **Workpad Notes**, and **Audit Events** are usable for real Tasker development work.
- **Dogfooding Readiness** requires enough init/config, queue setup, delegation or temporary task creation, one-shot work, local worktree handling, work updates, and status visibility to build Tasker with Tasker.
- **Dogfooding Readiness** uses single-worker execution only.
- **Bootstrap Task Creation** uses `tasker task create --bootstrap --queue <key> --file task.md`.
- A bootstrap task file uses YAML front matter for title, acceptance criteria, validation items, priority, state, tags, and review requirement, with the Markdown body as the **Task Brief**.
- **Bootstrap Task Creation** defaults to **Ready** when the task file does not specify state.
- **Bootstrap Task Creation** does not replace long-term agent-mediated intake.
- **Manual Dogfood Merge** may be used before automatic **Integrating** is implemented.
- **Manual Dogfood Merge** does not replace the full MVP requirement for Agent-Gated **Integrating** and **Squash Merge**.
- The **Tasker Service** owns **Tasks**, **Task Queues**, **Task States**, **Agent Runs**, and delivery records.
- The **Symphony Adapter** translates Symphony orchestration needs into **Tasker API** operations.
- The **Symphony Adapter** claims **Tasks**, prepares **Local Worktrees**, runs agents through an **Agent Launcher**, records work updates, and performs **Integrating** through a **Delivery Adapter**.
- **Pi Launcher** is the v1 **Agent Launcher**.
- **Pi Launcher** uses one fresh **Pi RPC Session** per **Agent Run**.
- **Pi Launcher** loads the **Tasker Pi Extension** for Tasker-aware Worker Agent runs.
- The dogfooding **Tasker Pi Extension** tool set includes task lookup, **Workpad Note** updates, requirement status updates, **Child Task** creation, and **State Transition** requests.
- The full **Tasker Pi Extension** exposes tools for Task context, **Workpad Note** updates, criteria/validation statuses, **Child Tasks**, **Task Links**, and **State Transitions**.
- Question UI is allowed in **Interactive Agent Sessions**.
- Unexpected question UI in an **Unattended Worker Session** fails the **Agent Run** with a clear reason.
- A **Pi Launcher** max-run timeout fails the **Agent Run** with a clear reason while preserving the **Run Transcript** and **Launcher Session Data**.
- Tasker may store a **Run Transcript** for each **Agent Run**.
- Tasker stores **Launcher Session Data** with common fields and launcher-specific raw data.
- Tasker does not automatically upload or share **Launcher Session Data**.
- **Workflow Metrics** are derived from Tasker events and run data rather than a separate metrics database.
- Tasker records **Agent Runs** and optional launcher metadata, not agent-protocol-specific control state.
- A **Task** may have many **Agent Runs** over its lifetime.
- An **Agent Run** owns one **Claim Lease** while its worker is alive.
- A **Worker Loop** sends a **Lease Heartbeat** every 30 seconds during an **Agent Run**.
- A **Claim Lease** expires after about 90 seconds without a **Lease Heartbeat**.
- Finishing an **Agent Run** records an **Agent Run Outcome** and releases its **Claim Lease**.
- Finishing an **Agent Run** does not directly change **Task State**.
- One **Agent Run** may cover execution and **Integrating** when it retains the **Claim Lease**.
- Claiming a **Ready** **Task** creates an **Agent Run** and moves the **Task** to **In Progress**.
- Claiming an **In Progress**, **Rework**, or **Integrating** **Task** creates an **Agent Run** without changing the **Task State**.
- An **Expired Agent Run** does not roll its **Task** back to **Ready**.
- A failed or **Expired Agent Run** may create a **Retry Hold**.
- A **Task** with an **Expired Agent Run** may be claimed again when its **Task State** is agent-eligible, it is not blocked, it has no active **Claim Lease**, and no **Retry Hold** is active.
- **Retry Holds** reset when **Task State** changes or an **Agent Run** completes successfully.
- Continuity across **Agent Runs** comes from **Local Worktree**, **Workpad Note**, structured Task data, and **Audit Events**, not reused chat history.
- Symphony executes agent processes; Tasker records **Agent Runs** and **Claim Leases** but does not run agents.

## Example dialogue

> **Dev:** "Should we implement teams, cycles, notifications, and a full Linear-style UI?"
> **Domain expert:** "No — the **Task Backend** only needs **Task Queues**, **Tasks**, **Task States**, and **Workpad Notes** for Symphony to run agents autonomously."

## Flagged ambiguities

- "task backend" could mean a general-purpose issue tracker or a narrow Symphony-compatible backend — resolved: it is the narrow backend, not a Linear clone.
- "Linear-compatible" could mean preserving Linear's API surface — resolved: Tasker exposes a first-class **Tasker API** instead of a fake Linear GraphQL facade.
- "eliminate Linear dependency" could imply importing or syncing Linear issues in v1 — resolved: v1 is greenfield local Tasker only; future imports use the **Tasker API**.
- "v1 backend" could mean API-only storage — resolved: v1 includes the **Tasker Service** plus a thin **Symphony Adapter** to validate the local workflow end-to-end.
- "MVP" could imply finishing every planned v1 feature before self-use — resolved: **Dogfooding Readiness** is an earlier milestone focused on using Tasker to build Tasker.
- "development loop until dogfooding" could mean either the temporary manual workflow or the Tasker-powered self-use workflow — resolved: use **Pre-Dogfooding Development Loop** before **Dogfooding Cutover**, then Tasker-managed dogfooding after cutover.
- "task" before Tasker can dogfood could conflict with the domain **Task** — resolved: use **Implementation Slice** for pre-dogfooding planning units and reserve **Task** for Tasker-managed work.
- "reviewer" during pre-dogfooding could mean either an advisory subagent or Tasker's domain **Review Agent** — resolved: use **Subagent Review Loop** for pre-dogfooding advisory review and reserve **Review Agent** for Tasker-managed review sessions.
- "manual merge" during dogfooding could redefine delivery — resolved: **Manual Dogfood Merge** is a temporary sequencing compromise, not the target delivery model.
- "serve" could imply workers run automatically — resolved: `tasker serve` only serves the API; `tasker work` runs the **Worker Loop** explicitly.
- "issue", "label", and "blocked_by" are Linear-shaped API terms — resolved: canonical Tasker language is **Task**, **Task Tag**, and **Blocking Task**.
- "project" could mean a Linear-style planning object or a routing key for agent work — resolved: use **Task Queue** for the routing key and avoid **Project** in v1.
- "scheduling" could imply due dates, estimates, milestones, or calendars — resolved: v1 scheduling is limited to queues, states, priority, blockers, leases, concurrency, and retry holds.
- "queue creation" could mean normal task delegation — resolved: **Task Queues** are **Operator**-managed infrastructure boundaries, not agent-created work data.
- "concurrency limit" could mean only Symphony's process limit — resolved: **Queue Concurrency Limit** is enforced by Tasker during claims, while Symphony may still enforce a global worker limit.
- "identifier" could mean internal database ID or human key — resolved: use an immutable UUID internally and **Task Identifier** for human/operator-facing references.
- "description" could mean an unstructured blob containing hidden requirements — resolved: use a **Task Brief** for narrative context and first-class **Acceptance Criteria** plus **Validation Items** for required outcomes/proof.
- "completion evidence" could live only in the **Workpad Note** — resolved: **Criterion Status**, **Validation Status**, and **Waivers** are structured Tasker data.
- "requirement edits" could weaken the task contract — resolved: Worker Agents may add/clarify requirements, but removals and waivers require Review Agent or Operator action.
- "waiver" could mean a worker self-exception — resolved: **Waivers** are created by **Review Agents** or **Operators** only.
- "priority" could mean arbitrary scoring or queue rank — resolved: use the fixed **Priority** labels urgent, high, normal, and low.
- "tag" could imply scheduling behavior — resolved: **Task Tags** are metadata only in v1.
- "PR link" could imply a GitHub dependency — resolved: v1 is local-first; Tasker stores generic typed **Task Links** such as **Local Worktree** paths and can add PR-like abstractions later.
- "backend" could mean Tasker itself or an integration strategy — resolved: **Task Backend** is Tasker; **Delivery Backend** is the pluggable work handoff/integration strategy.
- "Merging" could mean a specific Git/PR operation or a lifecycle state — resolved: use **Integrating** for the backend-neutral **Task State** and **Local Merge** for the v1 local-worktree operation.
- "integration failure" could mean bad work or bad infrastructure — resolved: use **Work-Change Delivery Failure** for Rework and **Operational Delivery Failure** for retry-in-Integrating.
- "Rework" could imply discarding the current attempt — resolved: local **Rework** revises the existing worktree unless **Reset Rework** is explicit.
- "no-change task" could imply skipping delivery — resolved: **Integrating** records **No-Change Integration** before **Done**.
- "validation passed" could become stale if **Main Branch** moves — resolved: record a **Validated Base Commit** and reject stale integration as a **Work-Change Delivery Failure**.
- "worktree changes" could imply uncommitted files are deliverable — resolved: **Integrating** requires committed **Task Commits** and a clean **Local Worktree**.
- "merge strategy" could imply preserving all agent iteration commits on **Main Branch** — resolved: default **Local Merge** is **Squash Merge**, one **Final Commit** per Task.
- "Delivery Backend" could imply Tasker executes Git or external commands — resolved: Tasker records delivery configuration/outcomes, while a Symphony-side **Delivery Adapter** performs operations.
- "local delivery config" could grow into build/test workflow config — resolved: **Local Worktree Delivery** config is limited to managed source repository, main branch, worktree root, branch template, and done-worktree retention.
- "agent prompt" could be hidden global behavior — resolved: use built-in **Role Prompts** with optional repo-owned **Prompt Overrides** under `.tasker/prompts/`.
- "local repo" could mean a personal working copy — resolved: **Local Worktree Delivery** uses a **Managed Source Repository** that the operator opts into Tasker/Symphony mutating.
- "Todo" could mean a generic task list label or an agent-eligible state — resolved: use **Ready** for queued agent work.
- "ready" could mean merely queued — resolved: **Ready** requires enough Acceptance Criteria and Validation Items for autonomous execution.
- "custom workflow" is out of scope for v1 — resolved: Tasker has a fixed **Task State** lifecycle first.
- "state update" could mean free-form mutation or a lifecycle event — resolved: normal clients use enforced **State Transitions**, while operators use **Repair Overrides** for exceptional fixes.
- "event log" could mean event sourcing — resolved: **Audit Events** are an audit log, while current records remain the authoritative read model.
- "In Progress" could mean a live process or an active work phase — resolved: **In Progress** is a **Task State**; **Agent Run** tracks live execution.
- "agent run" could mean Tasker launches agents — resolved: Tasker only records **Agent Runs** and **Claim Leases**; Symphony still executes agents through an **Agent Launcher**.
- "liveness" could mean agent output activity — resolved: **Lease Heartbeats** come from the **Worker Loop**, independent of Pi output.
- "v1 launcher" could default to the reference Codex app-server — resolved: the launcher is pluggable, but v1 ships **Pi Launcher**.
- "Pi integration" could mean SDK, JSON mode, print mode, or RPC — resolved: **Pi Launcher** uses **Pi RPC Sessions** for language-neutral streaming control.
- "session data" could mean pi-specific storage — resolved: use backend-neutral **Launcher Session Data** with common metrics plus raw launcher-specific artifacts.
- "session data" could also imply telemetry upload — resolved: **Launcher Session Data** is local-only by default and never automatically shared.
- "metrics" could imply a separate observability database — resolved: **Workflow Metrics** are derived from **Audit Events**, **Agent Runs**, **Launcher Session Data**, and **Integration Outcomes**.
- "agent questions" could stall unattended work — resolved: question UI is only for **Interactive Agent Sessions**, not **Unattended Worker Sessions**.
- "Tasker updates from pi" could mean shelling out through bash — resolved: Worker Agents use the **Tasker Pi Extension** for core workflow updates, with CLI reserved for operator/debug use.
- "local config" could mean the XDG default or a repository-local dogfooding config — resolved: the CLI warns on inactive project configs and refuses unsafe mutations unless the operator explicitly selects the intended config/data target.
- "retry continuity" could mean resuming hidden pi chat history — resolved: each **Agent Run** starts a fresh **Pi RPC Session**; durable continuity lives in Tasker/worktree data.
- "failed run retry" could mean immediate re-claim — resolved: failed or expired runs create **Retry Holds** with backoff.
- "task intake" could mean humans entering work directly — resolved: normal v1 intake is agent delegation through `tasker delegate`; humans delegate to agents rather than creating Tasks themselves.
- "bootstrap task creation" could become permanent manual intake — resolved: **Bootstrap Task Creation** is a temporary dogfooding shortcut only.
- "Backlog refinement" could mean manual field editing — resolved: use `tasker delegate --refine` and a **Delegation Interview**.
- "grill-me-with-docs" refers to the existing grill-with-docs-style interaction — resolved: call the Tasker intake flow a **Delegation Interview**.
- "documentation-aware delegation" could imply editing docs on the main repository during intake — resolved: delegation reads docs and creates Tasker work; repo edits happen in Worker Agent worktrees.
- "agent" could mean creator, executor, or reviewer — resolved: use **Delegating Agent**, **Worker Agent**, and **Review Agent** as distinct **Actor** roles.
- "identity" could mean API authentication or domain attribution — resolved: bearer tokens authenticate clients, while **Actor** records domain attribution.
- "human review" could imply humans operate Tasker directly — resolved: humans review through a **Review Session** or another external channel, and a **Review Agent** records the **Review Decision** in Tasker.
- "review required" could imply every Task blocks on a human — resolved: local-first queues default to **Agent-Gated Integration**, and **Human Review** is required only by **Review Policy** or explicit agent request.
- "workpad" could mean an ordinary comment to search for — resolved: **Workpad Note** is a first-class singleton with **Workpad Revisions**.
- "workpad checkbox" could imply authoritative completion state — resolved: structured Tasker fields are authoritative; the **Workpad Note** is narrative/handoff context.
- "root work" could imply manual Tasker entry — resolved: a **Root Task** is still created by a **Delegating Agent** from an out-of-band human request.
- "child task" could imply automatic dependency — resolved: parentage records delegation lineage only; blocking is explicit through a **Blocking Task** relationship.
- "cross-queue dependency" could imply multi-repo orchestration in v1 — resolved: **Child Tasks** and **Blocking Tasks** stay within one **Task Queue** for v1.
- "follow-up" could imply immediate agent pickup — resolved: a **Follow-up Task** starts in **Backlog** unless explicitly promoted to **Ready**.
- "resolved blocker" could mean any terminal state — resolved: only **Done** resolves a **Blocking Task**; **Canceled** does not.

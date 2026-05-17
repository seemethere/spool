# First-run Spool adoption journey for an existing local repository

This planning note maps the first successful journey for a developer adopting Spool in an existing local repository. It distinguishes discovery findings from implementation work; it does not require product behavior changes by itself. New adopters should start with the canonical quickstart in `docs/FIRST_RUN_QUICKSTART.md`; this document remains the discovery map behind it.

## Scope and assumptions

- Primary user: a developer who wants a local-first **Task Backend** for agent-driven development in an existing Git repository.
- Target happy path: initialize/configure Spool, create or select a **Task Queue**, plan **Tasks**, launch a **Worker Agent**, review results, and deliver completed work through **Local Worktree Delivery**.
- Desired product stance: CLI-first, local-first, no web UI, no GitHub or pull-request dependency, with the **Spool Pi Extension** as the preferred agent workflow surface.
- Current implementation is already useful for dogfooding, but the first-run path is still assembled from CLI help, ADRs, dogfood scripts, and feature-specific docs.

## End-to-end adoption journey

### 1. Decide that this repository is a Managed Source Repository

**User goal:** understand what Spool will own and what local repository mutations are allowed.

**Current path:** the developer learns from docs that a queue configured for **Local Worktree Delivery** makes the repository a **Managed Source Repository**. Spool/Symphony may create **Local Worktrees**, create and delete **Task Branches**, and eventually integrate work into the **Main Branch**.

**Friction and confusion:**

- This is an important trust boundary, but it is not presented as a first-run consent step.
- The term **Task Queue** is intentionally not **Project**, but a new adopter may still ask, “Do I need one queue per repository, team, or workstream?”
- Existing local uncommitted work is operationally important, yet the first-run path does not start with a visible repository safety checklist.

**Opportunity:** make the first command or guide say: “This repository will be a Managed Source Repository; Spool can mutate worktrees, branches, and Main Branch through the configured Delivery Adapter.”

### 2. Initialize local Spool state

**User goal:** create local config, data directory, SQLite database, and service token for this repository.

**Current path:** run `spool init`, optionally with `--config`, `--data-dir`, or `--db-path`. In this dogfood repository, `bin/spool-local` wraps those paths so commands target `.spool/config.toml` and `.spool/data`.

**Friction and confusion:**

- The generic default config path is user-scoped, while a repository adoption journey wants repo-local configuration by default or at least a clear recommendation.
- New users may not know whether to run `spool init` from the repository root, where the config will be written, or whether `.spool/` should be committed or ignored.
- The API token produced by init is necessary for `spool serve` plus the **Spool Pi Extension**, but the handoff between CLI config and extension environment is not surfaced as a single setup step.

**Opportunity:** provide a repository-first quickstart command sequence and explain which files are local state, which are safe to delete, and which should not be committed.

### 3. Create or select a Task Queue

**User goal:** define where Tasks are routed and how Local Worktree Delivery is configured.

**Current path:** run `spool queue create --key <KEY> --name <NAME> --managed-source-repository <PATH> --main-branch <BRANCH> --worktree-root <PATH> --branch-template <TEMPLATE>`. Optional flags include `--done-worktree-retention`, `--queue-concurrency-limit`, and `--actor`.

**Friction and confusion:**

- The required flags are accurate but dense; a first-time user must invent a **Task Queue Key**, branch template, worktree root, and Main Branch policy all at once.
- There is no obvious “use this repository with sane defaults” flow.
- The phrase **Task Queue** accurately avoids project-management semantics, but the setup form should still explain that this queue is the routing and delivery configuration for work in this repository.
- Queue creation is where the Managed Source Repository warning belongs, but the user currently has to infer it from docs.

**Opportunity:** add guided queue setup or a documented default: key from repository name, worktree root `.spool/worktrees`, branch template `spool/{task_identifier}`, Main Branch from Git, and Agent-Gated Integration unless `review_required` is chosen.

### 4. Start the Spool Service and connect agent tooling

**User goal:** make the **Spool API** available to the CLI, Worker Loop, and Spool Pi Extension.

**Current path:** run `spool serve`; the extension expects `SPOOL_API_URL`, `SPOOL_API_TOKEN`, actor environment variables, and optionally `SPOOL_AGENT_RUN_ID` / `SPOOL_WORKER_STATUS_PATH`.

**Friction and confusion:**

- Users must understand when they are using the CLI against SQLite directly versus using extension tools through the HTTP Spool API.
- The extension README lists environment variables, but there is no single copy-paste first-run export block tied to the config created by `spool init`.
- If `spool serve` is not running, pi extension failures look like agent/tooling problems rather than setup problems.

**Opportunity:** add a setup diagnostic such as “service reachable, token valid, extension env ready,” and include it in a first-run checklist.

### 5. Plan work into a Task

**User goal:** turn an intent into a Task with a clear Task Brief, structured Acceptance Criteria, structured Validation Items, priority, tags, conflict hints, and review policy.

**Current path:** preferred dogfood intake is an extension-native human-present **Delegation Session** using `spool_create_delegated_root_task`; `spool delegate` is a wrapper/fallback; file-backed Task Creation with `spool task create --queue <KEY> --from-file task.md` remains a compatibility path.

**Friction and confusion:**

- There are three intake paths, and their relative status is easy to miss: extension-native Delegation Session preferred, `spool delegate` fallback/wrapper, file-backed creation compatibility.
- New users need a template for good Acceptance Criteria and Validation Items; otherwise Tasks may be too vague for an unattended Worker Agent.
- The boundary between Task Brief narrative, Workpad Note handoff, Task Links, Task Conflict Hints, and structured gates is domain-specific and needs examples.

**Opportunity:** create a first Task wizard or quickstart template that produces one Ready Root Task and explains why structured gates matter.

### 6. Launch a Worker Agent

**User goal:** let a **Worker Loop** claim a Ready Task, create/use the Local Worktree and Task Branch, and run a Worker Agent through pi.

**Current path:** run `spool work --queue <KEY> --once --launcher pi` with the Spool Service available. The Worker Agent should use `spool_get_task_context_bundle`, update the Workpad Note, mark criteria and validation statuses, and request a State Transition through extension tools.

**Friction and confusion:**

- The `work` command exposes many operational flags, but the first-run happy path needs a minimal shape and a troubleshooting shape.
- The user must know that unattended Worker Sessions cannot ask questions; missing Task detail should be fixed during Delegation Session/refinement before work starts.
- If a Worker Agent fails because pi, extension configuration, service reachability, or task readiness is wrong, the recovery path spans `task show`, `run show`, Workpad Notes, and retry commands.

**Opportunity:** add a first-run worker smoke test that uses a tiny Task and validates service, extension, pi launcher, Local Worktree creation, and Agent Run recording before users trust it with meaningful code changes.

### 7. Observe progress and inspect results

**User goal:** understand what happened, what changed, and whether the Task gates passed.

**Current path:** use `spool status`, `spool monitor`, `spool task show <identifier>`, `spool run show <agent-run-id>`, Workpad Notes, Task Links, and Git inspection in the Local Worktree.

**Friction and confusion:**

- Observability is powerful but spread across multiple commands and concepts.
- A first-time user may not know which artifact is authoritative: Task State and structured requirement status are authoritative; Workpad Note is narrative; Task Links are references; Run Transcript is debugging data.
- The Local Worktree and Task Branch are visible as Task Links, but the user still needs clear “inspect this diff now” guidance.

**Opportunity:** provide a concise `spool next` or guide section for the post-run inspection order: Task gates, Workpad Note, Local Worktree diff, Agent Run outcome, Integration Outcome if present.

### 8. Review completed work

**User goal:** decide whether work should proceed through Agent-Gated Integration or require a human Review Session.

**Current path:** ordinary dogfood queues default to **Agent-Gated Integration**; Tasks with `review_required: true` enter **Human Review** and can use `spool review <task_identifier>` to record a Review Decision.

**Friction and confusion:**

- “Review” can mean advisory code review, Spool **Human Review**, or local Git inspection before integration. The first-run path needs to distinguish these.
- Users may assume every Task needs a pull request or human approval. That is intentionally not required for v1.
- A Human Review approve does not waive failing gates; this should be explicit in the first-run review story.

**Opportunity:** document two lanes: Agent-Gated Integration happy path for ordinary Tasks, and Human Review path only when the Review Policy requires it.

### 9. Integrate completed work and clean up

**User goal:** deliver the Task into Main Branch, record an Integration Outcome, and clean Local Worktree/Task Branch artifacts.

**Current path:** target v1 uses **Integrating** with Local Worktree Delivery and squash-style Final Commit. Current dogfood also documents Manual Dogfood Merge and runner-side helpers such as `spool merge integrate <task_identifier>` for already-Integrating Tasks.

**Friction and confusion:**

- The product direction is automatic Integrating, but dogfood docs still include temporary Manual Dogfood Merge. New adopters need to know which path is target and which is temporary.
- The clean-worktree and Validated Base Commit requirements are important, but they appear late in the journey.
- Users need confidence that Spool records outcomes even when no repository changes occur or integration fails.

**Opportunity:** make “what happens after gates pass” a first-class quickstart section: Task moves to Integrating, Delivery Adapter checks the Local Worktree, records Integration Outcome, moves to Done or Rework/retry, and cleans up local delivery artifacts after success.

## Cross-cutting UX gaps

1. **No single first-run adoption guide.** The journey currently spans ADRs, extension docs, dogfood merge docs, and CLI help.
2. **Repository-local configuration is not the obvious default.** Existing local repository adoption needs a clear `.spool/` story and safe path wrapper guidance.
3. **Managed Source Repository consent is under-emphasized.** Local branch/worktree/Main Branch mutation should be explicit before queue setup succeeds.
4. **Queue setup has too many first-contact decisions.** Good defaults could remove most flags for common one-repository adoption.
5. **Three Task intake paths compete for attention.** The preferred Delegation Session path should be visually primary; file-backed creation should read as compatibility/fallback.
6. **Extension setup lacks a readiness check.** The user has to manually connect init output, `spool serve`, API token, actor env, and pi extension loading.
7. **Readiness for unattended Worker Sessions is implicit.** Users need to know that vague Tasks cause failures or rework because Worker Agents cannot ask questions.
8. **Observability is fragmented for first-time users.** There is no “after the agent runs, inspect these five things in order” command or page.
9. **Review terminology is overloaded.** Advisory review, Human Review, Review Decision, and local Git inspection need a beginner-facing distinction.
10. **Target Integrating vs temporary Manual Dogfood Merge can blur.** The docs should clearly label temporary dogfood escape hatches and the intended v1 path.

## Prioritized follow-up Task candidates

| Order | Candidate Task | Rationale and mapped gap | Expected impact |
| --- | --- | --- | --- |
| 1 | Write a first-run existing-repository quickstart | Addresses gaps 1, 2, 3, 5, 8, and 10 by putting the whole happy path in one canonical guide using Spool domain language. | Highest demo value; gives new users and agents one path to follow without reading every ADR. |
| 2 | Add a repository adoption preflight/checklist command | Addresses gaps 2, 3, 4, and 6 by checking Git root, clean Main Branch, config/data paths, service reachability, token availability, queue presence, and extension env hints. | Reduces first-run setup failures before users launch agents. |
| 3 | Add guided local Task Queue setup defaults | Addresses gaps 3 and 4 by deriving Main Branch, worktree root, branch template, queue key suggestion, and Managed Source Repository warning for common repositories. | Removes the densest CLI step from the demo path while preserving explicit Operator control. |
| 4 | Create a first Task Delegation Session tutorial and template | Addresses gaps 5 and 7 by showing how human intent becomes a Ready Root Task with Task Brief, Acceptance Criteria, Validation Items, Conflict Hints, and review policy. | Improves Task quality and reduces unattended Worker Agent ambiguity. |
| 5 | Add Spool Pi Extension readiness diagnostics | Addresses gap 6 by validating `SPOOL_API_URL`, `SPOOL_API_TOKEN`, actor identity, `spool serve`, and extension tool registration before a Worker Loop or Delegation Session depends on them. | Converts confusing pi/tool failures into actionable setup feedback. |
| 6 | Add a minimal pi Worker Loop smoke journey | Addresses gaps 6, 7, and 8 with a tiny local Task that proves claim, Agent Run creation, context bundle access, Workpad update, and transition behavior. | Builds confidence before users entrust real repository changes to Spool. |
| 7 | Improve post-run inspection guidance or command output | Addresses gap 8 by ordering Task State, requirement statuses, Workpad Note, Agent Run, Task Links, Local Worktree diff, and Integration Outcome inspection. | Makes success/failure understandable after the first Worker Agent run. |
| 8 | Clarify review lanes in docs and prompts | Addresses gap 9 by distinguishing Agent-Gated Integration, Human Review, advisory Subagent Review Loop, and manual local Git inspection. | Prevents users from adding unnecessary Human Review or GitHub-shaped process. |
| 9 | Clarify Integrating vs temporary Manual Dogfood Merge in onboarding docs | Addresses gap 10 by labeling the target Local Worktree Delivery path and the dogfood escape hatch separately. | Keeps demos aligned with Spool's intended v1 local loop while acknowledging current dogfood reality. |
| 10 | Add example repository-local `.spool` ignore/config guidance | Addresses gap 2 by documenting what should be local-only, what can be regenerated, and how to avoid accidentally committing tokens or run data. | Reduces security and repository hygiene risk for adopters. |

## Suggested ordering

Start with documentation and diagnostics before changing workflow behavior:

1. First-run quickstart and terminology clarifications.
2. Repository adoption preflight/checklist.
3. Guided queue defaults.
4. Delegation Session tutorial/template.
5. Extension readiness and worker smoke journey.
6. Post-run inspection and review/integration polish.

This ordering gives immediate demo value while keeping behavior changes small and aligned with the existing **Dogfooding Readiness** roadmap.

## Validation notes

- Consistency check: the journey uses **Task Queue**, **Managed Source Repository**, **Local Worktree Delivery**, **Delegation Session**, **Worker Loop**, **Spool Pi Extension**, **Agent-Gated Integration**, **Human Review**, **Integrating**, and **Integration Outcome** as defined in `CONTEXT.md`.
- Roadmap check: recommendations support the documented v1 local loop: `spool init`, local-worktree Task Queue, delegated Root Task, `spool work --once`, pi Worker Agent, structured gate updates, Agent-Gated Integration, and local delivery.
- Planning-only check: the follow-up list is a backlog of candidate Tasks; this note does not change product behavior.

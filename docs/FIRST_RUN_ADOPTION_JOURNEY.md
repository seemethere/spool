# First-run Spool adoption journey for an existing local repository

This planning note maps the first successful journey for a developer adopting Spool in an existing local repository. It distinguishes discovery findings from implementation work; it does not require product behavior changes by itself. New adopters should start with the canonical quickstart in `docs/FIRST_RUN_QUICKSTART.md`; this document remains the discovery map behind it.

## Scope and assumptions

- Primary user: a developer who wants a local-first **Task Backend** for agent-driven development in an existing Git repository.
- Target happy path: initialize/configure Spool, create or select a **Task Queue**, plan **Tasks**, launch a **Worker Agent**, review results, and deliver completed work through **Local Worktree Delivery**.
- Desired product stance: CLI-first, local-first, no web UI, no GitHub or pull-request dependency, with the **Spool Pi Extension** as the preferred agent workflow surface.
- Current implementation is already useful for dogfooding. The first-run path now has a canonical quickstart, review/integration lane guidance, repository-local state hygiene guidance, and temporary Manual Dogfood Merge documentation, but several setup diagnostics and guided defaults remain follow-up work.

## End-to-end adoption journey

### 1. Decide that this repository is a Managed Source Repository

**User goal:** understand what Spool will own and what local repository mutations are allowed.

**Current path:** the developer learns from docs that a queue configured for **Local Worktree Delivery** makes the repository a **Managed Source Repository**. Spool/Symphony may create **Local Worktrees**, create and delete **Task Branches**, and eventually integrate work into the **Main Branch**.

**Current status:** `docs/FIRST_RUN_QUICKSTART.md` now opens with an explicit Managed Source Repository warning and a small `git status` / current-branch safety check. It explains that Spool/Symphony tooling may mutate Local Worktrees, Task Branches, Main Branch, and cleanup artifacts.

**Remaining friction and confusion:**

- The term **Task Queue** is intentionally not **Project**, but a new adopter may still ask, “Do I need one queue per repository, team, or workstream?”
- The warning is documentation-only; queue setup still does not have a guided consent or preflight command.

**Opportunity:** add a repository adoption preflight or guided queue setup step that repeats the Managed Source Repository warning before Spool starts mutating local delivery artifacts.

### 2. Initialize local Spool state

**User goal:** create local config, data directory, SQLite database, and service token for this repository.

**Current path:** run `spool init`, optionally with `--config`, `--data-dir`, or `--db-path`. In this dogfood repository, `bin/spool-local` wraps those paths so commands target `.spool/config.toml` and `.spool/data`.

**Current status:** `docs/FIRST_RUN_QUICKSTART.md` gives the repository-local command shape and `docs/REPOSITORY_LOCAL_STATE.md` explains local state, cleanup commands, and conservative `.gitignore` guidance.

**Remaining friction and confusion:**

- The generic default config path is still user-scoped, so adopters must deliberately choose repo-local `--config .spool/config.toml --data-dir .spool/data` or equivalent environment.
- The API token produced by init is necessary for `spool serve` plus the **Spool Pi Extension**, but there is no single readiness check that proves the CLI config, service, token, actor environment, and extension tooling are aligned.

**Opportunity:** add a setup diagnostic that verifies repository-local config/data paths, service reachability, token validity, and extension env hints.

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

**Current status:** The quickstart makes the **Delegation Session** path primary, labels file-backed creation and the older bootstrap spelling as temporary or compatibility escape hatches, and links to `docs/TASK_DELEGATION_SESSION_TUTORIAL.md`.

**Remaining friction and confusion:**

- New users still need richer examples for good Acceptance Criteria and Validation Items; otherwise Tasks may be too vague for an unattended Worker Agent.
- The boundary between Task Brief narrative, Workpad Note handoff, Task Links, Task Conflict Hints, and structured gates is domain-specific and benefits from more examples.

**Opportunity:** expand the first Task tutorial/template so it produces one Ready Root Task and explains why structured gates matter.

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

**Current status:** The quickstart now includes a post-run inspection order: queue/task state, structured gates, Workpad Note, Task Links, Local Worktree diff, Agent Run, and Integration Outcome. It also warns not to paste raw transcripts, prompt bodies, secrets, or large raw logs into handoff surfaces.

**Remaining friction and confusion:**

- Observability is still spread across multiple commands and concepts.
- The Local Worktree and Task Branch are visible as Task Links, but a dedicated command could still make “inspect this diff now” more obvious.

**Opportunity:** improve command output or add a concise guided inspection command that follows the documented order.

### 8. Review completed work

**User goal:** decide whether work should proceed through Agent-Gated Integration or require a human Review Session.

**Current path:** ordinary dogfood queues default to **Agent-Gated Integration**; Tasks with `review_required: true` enter **Human Review** and can use `spool review <task_identifier>` to record a Review Decision.

**Current status:** `docs/REVIEW_AND_INTEGRATION_LANES.md` and the quickstart distinguish Agent-Gated Integration, Human Review with a Review Decision, advisory Subagent Review Loop, local Git inspection, and temporary Manual Dogfood Merge. They also state that human approval or advisory feedback does not waive failed gates.

**Remaining friction and confusion:**

- These distinctions still rely on a reader following cross-links rather than a command explaining the next lane for a specific Task.

**Opportunity:** keep these lane definitions visible in prompts and future inspection/status output.

### 9. Integrate completed work and clean up

**User goal:** deliver the Task into Main Branch, record an Integration Outcome, and clean Local Worktree/Task Branch artifacts.

**Current path:** target v1 uses **Integrating** with Local Worktree Delivery and squash-style Final Commit. Current dogfood also documents Manual Dogfood Merge and runner-side helpers such as `spool merge integrate <task_identifier>` for already-Integrating Tasks.

**Current status:** The quickstart now separates target Agent-Gated Integration from the temporary Manual Dogfood Merge escape hatch, and `docs/REVIEW_AND_INTEGRATION_LANES.md` repeats that distinction.

**Remaining friction and confusion:**

- The clean-worktree and Validated Base Commit requirements remain important enough to reinforce in command output.
- Users need confidence that Spool records outcomes even when no repository changes occur or integration fails.

**Opportunity:** keep “what happens after gates pass” visible in first-run output: Task moves to Integrating, the Delivery Adapter checks the Local Worktree, records an Integration Outcome, moves to Done or Rework/retry, and cleans up local delivery artifacts after success.

## Current adoption status and remaining UX gaps

### Completed or partially completed documentation work

1. **Canonical first-run quickstart exists.** `docs/FIRST_RUN_QUICKSTART.md` is now the first link for the existing-repository happy path.
2. **Repository-local state hygiene is documented.** `docs/REPOSITORY_LOCAL_STATE.md` explains local Spool state, cleanup expectations, and conservative ignore guidance.
3. **Post-run inspection order is documented.** The quickstart orders authoritative structured state before Workpad Notes, Task Links, Local Worktree diff, Agent Run metadata, and Integration Outcomes.
4. **Review and integration lanes are documented.** `docs/REVIEW_AND_INTEGRATION_LANES.md` distinguishes Agent-Gated Integration, Human Review, advisory Subagent Review Loop, local Git inspection, and temporary Manual Dogfood Merge.
5. **Target Integrating vs temporary Manual Dogfood Merge is labeled.** The quickstart and review/integration lane guide both mark Manual Dogfood Merge as a dogfooding escape hatch rather than the target delivery model.

### Remaining gaps

1. **Repository-local configuration is still explicit rather than guided.** The docs recommend `.spool/config.toml` and `.spool/data`, but product defaults and diagnostics still require care.
2. **Managed Source Repository consent is documentation-only.** Queue setup should eventually make branch/worktree/Main Branch mutation risks more visible in command output or preflight checks.
3. **Queue setup has too many first-contact decisions.** Good defaults could remove most flags for common one-repository adoption.
4. **Extension setup lacks a readiness check.** The user has to manually connect init output, `spool serve`, API token, actor env, and pi extension loading.
5. **Readiness for unattended Worker Sessions is implicit.** Users need examples showing that vague Tasks should be refined before a Worker Agent is launched.
6. **Observability remains multi-command.** The documented inspection order helps, but a command could still summarize next actions for a specific Task.
7. **Integration preconditions should be surfaced earlier in tooling.** Clean Local Worktree, committed Task Commits, and current-enough base requirements should be obvious before users request Integrating.

## Prioritized follow-up Task candidates

| Order | Candidate Task | Status | Rationale and mapped gap | Expected impact |
| --- | --- | --- | --- | --- |
| 1 | Add a repository adoption preflight/checklist command | Still open | Addresses remaining gaps 1, 2, 4, and 7 by checking Git root, clean Main Branch, config/data paths, service reachability, token availability, queue presence, extension env hints, and integration preconditions. | Reduces first-run setup failures before users launch agents. |
| 2 | Add guided local Task Queue setup defaults | Still open | Addresses remaining gaps 2 and 3 by deriving Main Branch, worktree root, branch template, queue key suggestion, and Managed Source Repository warning for common repositories. | Removes the densest CLI step from the demo path while preserving explicit Operator control. |
| 3 | Expand the first Task Delegation Session tutorial and template | Partially started | Addresses remaining gap 5 by showing how human intent becomes a Ready Root Task with Task Brief, Acceptance Criteria, Validation Items, Conflict Hints, and review policy. | Improves Task quality and reduces unattended Worker Agent ambiguity. |
| 4 | Add Spool Pi Extension readiness diagnostics | Still open | Addresses remaining gap 4 by validating `SPOOL_API_URL`, `SPOOL_API_TOKEN`, actor identity, `spool serve`, and extension tool registration before a Worker Loop or Delegation Session depends on them. | Converts confusing pi/tool failures into actionable setup feedback. |
| 5 | Add a minimal pi Worker Loop smoke journey | Still open | Addresses remaining gaps 4, 5, and 6 with a tiny local Task that proves claim, Agent Run creation, context bundle access, Workpad update, and transition behavior. | Builds confidence before users entrust real repository changes to Spool. |
| 6 | Improve post-run inspection command output | Documentation exists; command polish open | Addresses remaining gap 6 by ordering Task State, requirement statuses, Workpad Note, Agent Run, Task Links, Local Worktree diff, and Integration Outcome inspection in one guided surface. | Makes success/failure understandable after the first Worker Agent run. |
| 7 | Surface review/integration lane hints in prompts and status output | Documentation exists; prompt/tooling polish open | Keeps Agent-Gated Integration, Human Review, advisory Subagent Review Loop, local Git inspection, and Manual Dogfood Merge distinctions visible at the point of use. | Prevents unnecessary Human Review or GitHub-shaped process. |

## Suggested ordering

Start with diagnostics and guided setup before changing workflow behavior:

1. Repository adoption preflight/checklist.
2. Guided queue defaults.
3. Extension readiness and worker smoke journey.
4. Delegation Session tutorial/template expansion.
5. Post-run inspection and review/integration command/prompt polish.

This ordering builds on the completed documentation and gives immediate demo value while keeping behavior changes small and aligned with the existing **Dogfooding Readiness** roadmap.

## Validation notes

- Consistency check: the journey uses **Task Queue**, **Managed Source Repository**, **Local Worktree Delivery**, **Delegation Session**, **Worker Loop**, **Spool Pi Extension**, **Agent-Gated Integration**, **Human Review**, **Integrating**, and **Integration Outcome** as defined in `CONTEXT.md`.
- Roadmap check: recommendations support the documented v1 local loop: `spool init`, local-worktree Task Queue, delegated Root Task, `spool work --once`, pi Worker Agent, structured gate updates, Agent-Gated Integration, and local delivery.
- Planning-only check: the follow-up list is a backlog of candidate Tasks; this note does not change product behavior.

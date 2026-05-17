# First-run quickstart for an existing repository

This guide is the canonical first link for adopting Spool in an existing local Git repository. It keeps the happy path local-first and CLI-first: initialize Spool, configure one **Task Queue** for **Local Worktree Delivery**, create a structured **Root Task** through a **Delegation Session**, run a **Worker Loop**, inspect the result, and let **Agent-Gated Integration** deliver the work.

Spool v1 does not require a web UI, GitHub, pull requests, external tracker sync, or custom workflow fields.

## 0. Decide what Spool may mutate

A **Task Queue** configured for **Local Worktree Delivery** makes this repository a **Managed Source Repository**. That means Spool/Symphony tooling may create **Local Worktrees**, create and delete **Task Branches**, perform a local **Squash Merge** into the configured **Main Branch**, and clean local delivery artifacts after successful integration.

Before starting:

```bash
git status --short
git branch --show-current
```

Use a repository whose **Main Branch** you are comfortable letting Spool-managed tooling mutate. Commit, stash, or move unrelated local work before launching a **Worker Loop**.

## 1. Build or install Spool

From a source checkout, verify the CLI builds:

```bash
cargo run -p spool-cli --bin spool -- --help
```

If you installed Spool another way, replace `cargo run -p spool-cli --bin spool --` in the examples with `spool`.

## 2. Initialize repository-local Spool state

Run from the repository root:

```bash
spool init --config .spool/config.toml --data-dir .spool/data
```

`spool init` creates local config, a local data directory, the SQLite Task Backend, and an API token. Treat `.spool/config.toml`, `.spool/data/`, API tokens, **Run Transcripts**, **Launcher Session Data**, and **Local Worktrees** as local Operator state unless your repository has a deliberate policy for sharing sanitized examples. See [Repository-local Spool state hygiene](REPOSITORY_LOCAL_STATE.md) for artifact expectations, cleanup commands, and `.gitignore` guidance.

## 3. Create a Task Queue for Local Worktree Delivery

Choose a short **Task Queue Key** and a **Main Branch**. This example uses `APP`, `main`, and local worktrees under `.spool/worktrees`:

```bash
spool queue create \
  --key APP \
  --name "App Local Work" \
  --managed-source-repository "$PWD" \
  --main-branch main \
  --worktree-root .spool/worktrees \
  --branch-template 'spool/{task_identifier}'
```

The **Task Queue** is the routing and delivery boundary for Tasks in this repository. It is not a Linear-style project, team, milestone, or external tracker sync.

## 4. Start the Spool Service

Start the HTTP **Spool Service** in one terminal:

```bash
spool serve --config .spool/config.toml --data-dir .spool/data
```

The service owns **Tasks**, **Task States**, **Agent Runs**, and delivery records. It does not secretly run workers; agent execution starts only when you run a **Worker Loop**.

For pi sessions that load the **Spool Pi Extension**, set the service URL, token, and actor environment expected by `extensions/spool-pi/src/index.ts`:

```bash
export SPOOL_API_URL=http://127.0.0.1:4317
export SPOOL_API_TOKEN=<token from spool init/config>
export SPOOL_ACTOR_KIND=delegating_agent
export SPOOL_ACTOR_ID=local-delegator
export SPOOL_ACTOR_DISPLAY_NAME="Local Delegating Agent"
```

## 5. Create the first Root Task through a Delegation Session

Preferred intake is a human-present **Delegation Session** in pi with the **Spool Pi Extension** loaded. Ask the **Delegating Agent** to run a one-question-at-a-time **Delegation Interview** and create one **Root Task** with `spool_create_delegated_root_task`. For a step-by-step example, see `docs/TASK_DELEGATION_SESSION_TUTORIAL.md`.

A good first Task has:

- a concise **Task Brief**;
- at least one structured **Acceptance Criterion**;
- at least one structured **Validation Item**;
- priority `urgent`, `high`, `normal`, or `low`;
- optional **Task Tags**, **Task Conflict Hints**, and same-queue **Blocking Tasks**;
- `review_required: false` unless this Task or queue really needs **Human Review**.

The created **Root Task** starts in **Backlog** by default. It may start in **Ready** only when the structured gates are sufficient for an unattended **Worker Agent**.

`spool delegate` is a CLI wrapper/fallback around the same Delegation Session contract:

```bash
spool delegate --queue APP "Create a tiny change that proves Spool can run in this repository"
```

## 6. Temporary and compatibility intake escape hatches

Use these only when the Delegation Session path is unavailable or while dogfooding transitional behavior:

- **File-backed Task Creation** is the compatibility path for creating a Task from a Markdown file with YAML front matter:

  ```bash
  spool task create --queue APP --from-file task.md
  ```

- The older `--bootstrap --file` spelling is the **Bootstrap Task Creation** compatibility path:

  ```bash
  spool task create --bootstrap --queue APP --file task.md
  ```

These commands do not replace the target intake model. The target path is **Delegation Session** -> structured Spool fields -> **Ready** Task when gates are adequate.

## 7. Launch one Worker Loop run

Keep `spool serve` running, then launch one **Worker Loop** claim from another terminal:

```bash
spool work --config .spool/config.toml --data-dir .spool/data \
  --queue APP \
  --once \
  --launcher pi \
  --api-url http://127.0.0.1:4317 \
  --pi-extension extensions/spool-pi/src/index.ts
```

The **Worker Loop** claims an eligible **Ready** Task, creates an **Agent Run** with a **Claim Lease**, prepares the **Local Worktree** and **Task Branch**, then launches a pi-backed **Worker Agent**. The Worker Agent should use `spool_get_task_context_bundle` first, update the **Workpad Note**, mark structured criteria and validation statuses, attach useful **Task Links**, and request the next **Task State** through Spool Pi Extension tools.

Unattended Worker Sessions cannot stop to ask the human questions. If the Task is ambiguous, refine it in a **Delegation Session** before launching work.

## 8. Inspect the result

After a **Worker Loop** exits, inspect from the most authoritative Spool state toward progressively more detailed local debugging artifacts. This order helps decide whether the Task is ready for **Agent-Gated Integration**, needs **Rework**, or needs operator recovery without dumping raw transcripts.

1. Start with queue-level attention and current **Task State**:

   ```bash
   spool status --config .spool/config.toml --data-dir .spool/data
   spool task show <task_identifier> --config .spool/config.toml --data-dir .spool/data
   ```

   **Task State** and the structured gate statuses are the first source of truth. A Task in **In Progress** may still have an active or stuck **Agent Run**; a Task in **Rework** needs another Worker Agent pass or explicit operator handling; a Task in **Integrating** is in delivery; a Task in **Done** is complete in Spool.

2. Read the structured **Acceptance Criteria** and **Validation Items** in `spool task show`. These are authoritative completion gates: every Acceptance Criterion must be `satisfied` or `waived`, and every Validation Item must be `passed` or `waived`, before ordinary Agent-Gated Integration should proceed. **Workpad Note** checkboxes or prose can explain this state, but Spool does not treat Markdown as authoritative gate state.

3. Read the **Workpad Note** for narrative handoff context: what changed, which commands ran, known risks, follow-up Task candidates, and any efficiency notes from the Worker Agent. Use it to understand intent and evidence, not to override structured requirements.

4. Inspect **Task Links** to find delivery references, especially the **Local Worktree** path and **Task Branch**. Task Links are references to artifacts; they do not replace the Task Brief, structured requirements, or validation status.

5. Inspect the **Local Worktree** and **Task Branch** recorded as Task Links:

   ```bash
   cd <local_worktree_path>
   git status --short
   git diff --stat <main-branch>...HEAD
   git log --oneline <main-branch>..HEAD
   ```

   The Local Worktree diff and Task Commits show the repository changes that delivery will integrate. Before requesting or trusting Integrating, the Local Worktree should be clean and intended changes should be committed on the Task Branch.

6. Inspect the latest **Agent Run** outcome and concise launcher metadata:

   ```bash
   spool run show <agent_run_id> --config .spool/config.toml --data-dir .spool/data
   ```

   Prefer the latest completed Agent Run for handoff evidence, but scan earlier failed or expired runs when warnings remain unexplained. `spool run show` should be enough for normal diagnosis: outcome, failure reason, timing, local Run Transcript path, and normalized **Launcher Session Data** or efficiency summaries. Open the saved **Run Transcript** only when the summary is insufficient to explain a failure.

7. If the Task reached **Integrating** or **Done**, inspect the latest **Integration Outcome** through `spool task show`, `spool status`, `spool run show`, or the temporary merge helpers when applicable. Integration Outcomes record whether Local Worktree Delivery succeeded, produced no changes, found a work-change failure such as a dirty worktree or merge conflict, or hit a retryable operational failure. For the target path, the Delivery Adapter records the Integration Outcome and moves the Task to Done, Rework, or retry-in-Integrating as appropriate.

Authoritative Spool data is structured: **Task State**, **Acceptance Criteria**, **Validation Items**, **Agent Runs**, **Integration Outcomes**, and Audit Events. Narrative and reference surfaces are still important but different: the **Task Brief** explains the request, the **Workpad Note** is handoff context, and **Task Links** point to artifacts such as the Local Worktree and Task Branch. Debugging artifacts are local by default: **Run Transcripts**, raw **Launcher Session Data**, prompt bodies, tool arguments, secrets, and large raw logs must not be pasted into Workpad Notes, commit messages, documentation, or external systems unless an Operator intentionally creates a sanitized diagnostic excerpt.

For current dogfooding, **Manual Dogfood Merge** may still be used as a temporary escape hatch when automatic Integrating is unavailable; see `docs/MANUAL_DOGFOOD_MERGE.md` for the longer operator checklist. The target Local Worktree Delivery behavior remains the Agent-Gated Integration path in the next section.

## 9. Integrate through the target path

For ordinary local-first queues, the default review policy is **Agent-Gated Integration**. When all Acceptance Criteria are satisfied or waived and all Validation Items are passed or waived, the Worker Agent may request **Integrating** without a human review step unless the Task or queue requires **Human Review**.

Target delivery path:

1. Task enters **Integrating**.
2. The runner-side **Delivery Adapter** checks that the **Local Worktree** is clean and contains committed **Task Commits** on the **Task Branch**.
3. Local Worktree Delivery verifies the branch is current enough for the configured **Main Branch**.
4. The Delivery Adapter performs a local **Squash Merge**, records an **Integration Outcome**, and moves the Task to **Done**, **Rework**, or retry-in-**Integrating** as appropriate.
5. On success, the **Main Branch** receives one **Final Commit** and local delivery artifacts are cleaned unless retention is configured.

## 10. Temporary Manual Dogfood Merge escape hatch

**Manual Dogfood Merge** is temporary dogfooding guidance, not the target delivery model. Use it only when automatic **Integrating** is unavailable or an operator deliberately chooses the compatibility path.

The temporary helper for an already-**Integrating** Task is:

```bash
spool merge integrate <task_identifier> --config .spool/config.toml --data-dir .spool/data
```

For a fully manual merge, acquire the **Managed Source Repository Operation Lock**, inspect the Task and **Agent Run**, validate from the **Local Worktree**, perform an operator-side squash-style **Local Merge** into the **Main Branch**, run post-merge validation from the Managed Source Repository, then record completion through supported Spool gates. See `docs/MANUAL_DOGFOOD_MERGE.md` for the detailed temporary checklist.

## 11. Troubleshooting checklist

- `spool serve` must be running before pi extension tools can reach the **Spool API**.
- Use explicit `--config .spool/config.toml --data-dir .spool/data` until your shell environment makes the intended Task Backend unambiguous.
- A Task must have enough structured gates before it can be **Ready** for unattended work.
- A dirty **Managed Source Repository** or held **Managed Source Repository Operation Lock** can prevent new claims.
- A dirty **Local Worktree**, missing **Task Commits**, stale validation base, or merge conflict is work for **Rework**, not a reason to invent a pull-request workflow.
- Use `spool task retry` for failed or stuck agent work after the cause is fixed. Use `spool merge retry` only for retryable **Operational Delivery Failure** while a Task remains **Integrating**.

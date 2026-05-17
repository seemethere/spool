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

`spool init` creates local config, a local data directory, the SQLite Task Backend, and an API token. Treat `.spool/config.toml`, `.spool/data/`, API tokens, **Run Transcripts**, and **Launcher Session Data** as local operator state unless your repository has a deliberate policy for sharing sanitized examples.

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

Preferred intake is a human-present **Delegation Session** in pi with the **Spool Pi Extension** loaded. Ask the **Delegating Agent** to run a one-question-at-a-time **Delegation Interview** and create one **Root Task** with `spool_create_delegated_root_task`.

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

Use this order after the Worker Loop exits:

```bash
spool status --config .spool/config.toml --data-dir .spool/data
spool task show <task_identifier> --config .spool/config.toml --data-dir .spool/data
spool run show <agent_run_id> --config .spool/config.toml --data-dir .spool/data
```

Then inspect the **Local Worktree** and **Task Branch** recorded as **Task Links**:

```bash
cd <local_worktree_path>
git status --short
git diff --stat <main-branch>...HEAD
git log --oneline <main-branch>..HEAD
```

Authoritative completion gates live in structured **Acceptance Criteria** and **Validation Items**. The **Workpad Note** is narrative handoff context. **Run Transcripts** and **Launcher Session Data** are local debugging artifacts; do not paste raw transcripts, prompt bodies, secrets, or large logs into commit messages or docs.

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

# Local Review Sessions

`spool review <task_identifier>` starts a Pi-backed **Review Session** for a single **Task** in **Human Review**. The command builds the local **Review Packet**, launches a **Review Agent** in the configured Managed Source Repository, and passes the packet as session context.

The Review Agent uses the built-in Review Agent Role Prompt unless the Managed Source Repository provides `.spool/prompts/review.md`. The Review Agent should summarize the Review Packet, ask the present human for one explicit **Review Decision** (`approve` or `rework`), collect concise feedback for rework, and record the decision through the deterministic review-decision path exposed by the Spool API / Spool Pi Extension.

## Human-present happy path

Use this flow when a **Task** is already in **Human Review** because the **Review Policy** requires it or an agent explicitly requested human review:

1. From the Managed Source Repository, run `spool review <task_identifier>` with the intended project Spool config selected.
2. Spool loads the **Task**, **Workpad Note**, structured **Acceptance Criteria** and **Validation Items**, **Task Links**, delivery metadata, and local Git summary for the **Local Worktree**.
3. The command renders a concise **Review Packet** for the **Review Agent**. The packet includes the **Task Identifier**, title, **Task State**, **Task Queue**, priority, review requirement, **Task Brief** summary, **Workpad Note** handoff, requirement statuses, relevant **Task Links**, **Task Branch**, **Task Commits**, diff summary, known risks, and reviewer attention notes.
4. The Pi-backed **Review Agent** presents or summarizes the **Review Packet** and asks the present human to choose exactly one **Review Decision**:
   - `approve` means the human accepts the work.
   - `rework` means the human wants the existing work revised and must provide concise feedback.
5. The **Review Agent** records the **Review Decision** with Review Agent actor attribution. This writes Audit Events and performs the resulting **State Transition**.

For an approve **Review Decision**, the **Task State** moves from **Human Review** to **Integrating**. The approved work is still subject to **Local Worktree Delivery**: the **Local Worktree** must be clean, changes must be committed as **Task Commits** on the **Task Branch**, and delivery may still record an **Integration Outcome** such as a **Work-Change Delivery Failure** or **Operational Delivery Failure**.

For a rework **Review Decision**, the **Task State** moves from **Human Review** to **Rework**. Human feedback is recorded as Review Decision context and summarized in the **Workpad Note** so a future **Worker Agent** can revise the existing **Local Worktree** by default.

Approving does not imply waivers. Any unsatisfied **Acceptance Criterion** or unpassed **Validation Item** must be satisfied, passed, or explicitly waived by a **Review Agent** or **Operator** before a **Task** can leave **Human Review** for **Integrating**.

Question UI is expected in this **Interactive Agent Session**. This does not change **Unattended Worker Session** behavior: blocking question UI during `spool work --launcher pi` still fails the **Agent Run** with the `unattended_question` failure reason code instead of waiting for a human.

The first Review Session path is local-first and does not require GitHub, a pull request, or a web UI. The Review Packet intentionally omits raw Run Transcript bodies, raw Launcher Session Data payloads, prompts, secrets, and unrelated Task Queue data.


## Task Link attachment during review

A **Review Agent** may attach or update **Task Links** through the **Spool Pi Extension** when the review produces a durable reference that will help rework, handoff, or operator inspection. Common examples are a local Review Packet artifact, a diff or patch file, a local log excerpt artifact, or an optional external review/reference target. Use `is_primary` only when the artifact is the main item a future reviewer or finishing agent should inspect first.

Task Links are collaboration and delivery references, not authoritative gate state. The actual **Review Decision** must be recorded through the review-decision path, and structured **Acceptance Criteria**, **Validation Items**, and any explicit **Waivers** remain authoritative for whether a **Task** may leave **Human Review** for **Integrating**.

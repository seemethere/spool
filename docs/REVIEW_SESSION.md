# Local Review Sessions

`tasker review <task_identifier>` starts a Pi-backed **Review Session** for a single **Task** in **Human Review**. The command builds the local **Review Packet**, launches a **Review Agent** in the configured Managed Source Repository, and passes the packet as session context.

The Review Agent uses the built-in Review Agent Role Prompt unless the Managed Source Repository provides `.tasker/prompts/review.md`. The Review Agent should summarize the Review Packet, ask the present human for one explicit **Review Decision** (`approve` or `rework`), collect concise feedback for rework, and record the decision through the deterministic review-decision path exposed by the Tasker API / Tasker Pi Extension.

Question UI is expected in this **Interactive Agent Session**. This does not change **Unattended Worker Session** behavior: blocking question UI during `tasker work --launcher pi` still fails the **Agent Run** with the `unattended_question` failure reason code instead of waiting for a human.

The first Review Session path is local-first and does not require GitHub, a pull request, or a web UI. The Review Packet intentionally omits raw Run Transcript bodies, raw Launcher Session Data payloads, prompts, secrets, and unrelated Task Queue data.

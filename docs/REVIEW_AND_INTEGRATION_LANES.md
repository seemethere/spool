# Review and integration lanes

This guide is beginner-facing onboarding guidance for the words around review and delivery. Spool keeps ordinary local-first work out of mandatory human bottlenecks while still allowing explicit **Human Review** when a **Review Policy**, **Task**, or human/**Operator** asks for it.

## The ordinary lane: Agent-Gated Integration

Most local-first **Task Queues** should use **Agent-Gated Integration** by default. In that lane:

1. A **Worker Agent** completes the **Task** in a **Local Worktree** and commits focused **Task Commits** on the **Task Branch**.
2. The Worker Agent marks each **Acceptance Criterion** satisfied or waived and each **Validation Item** passed or waived, with evidence in structured Spool fields and a concise **Workpad Note** handoff.
3. When the structured gates pass, the Worker Agent may request **Integrating** without a human review step, unless the queue or Task requires **Human Review**.
4. In **Integrating**, runner-side **Local Worktree Delivery** checks the committed work, performs the local **Squash Merge** into the **Main Branch**, records an **Integration Outcome**, and moves the Task to **Done**, **Rework**, or retry-in-**Integrating** as appropriate.

Agent-Gated Integration is not “ungated auto-merge.” The structured **Acceptance Criteria**, **Validation Items**, clean **Local Worktree**, committed **Task Commits**, delivery checks, and **Integration Outcome** remain the gates.

## The Human Review lane

Use **Human Review** only when the **Review Policy** requires it, the Task sets `review_required: true`, an agent explicitly requests it, or a human/**Operator** asks for it.

In this lane, `spool review <task_identifier>` starts a local **Review Session** with a **Review Agent**. The Review Agent prepares a **Review Packet**, asks the present human for one explicit **Review Decision**, and records that decision in Spool:

- approve: move from **Human Review** to **Integrating**;
- rework: move from **Human Review** to **Rework** with feedback.

Approval does not waive failed gates. Before a Task can leave Human Review for Integrating, every Acceptance Criterion must be satisfied or explicitly waived, and every Validation Item must be passed or explicitly waived. Waivers are separate Spool mutations with reasons; they are not implied by a human saying “approved.”

This lane is local-first. It does not require GitHub, pull requests, a web dashboard, or a permanent human checkpoint for ordinary Tasks.

## Advisory review is not Spool Human Review

A **Subagent Review Loop** is advisory development help. A Worker Agent may ask reviewer subagents to inspect a plan or diff before committing or requesting Integrating, especially for risky changes.

That advisory loop does not put the Task into Human Review. It does not create a Review Session, does not use a Review Agent, does not prepare the official Review Packet, and does not record a Review Decision. The Worker Agent still remains responsible for updating structured gates, Workpad Note handoff context, and the next State Transition.

## Local Git inspection is not a Review Decision

Local Git inspection is an operator or agent looking at the **Local Worktree**, **Task Branch**, diff, commits, and validation output. It is useful before delivery and during troubleshooting, but by itself it is not a Spool Review Decision.

For Agent-Gated Integration, local Git inspection supports delivery checks and confidence before Integrating. For Human Review, local Git inspection can inform the human decision, but the Review Agent must still record the Review Decision through Spool.

## Manual Dogfood Merge is temporary

**Manual Dogfood Merge** is a dogfooding escape hatch for periods when automatic Integrating is unavailable or an operator deliberately chooses the compatibility path. It lets an operator inspect a completed Local Worktree and perform an operator-side squash-style Local Merge outside the **Spool Service**.

Manual Dogfood Merge does not replace the target lane. The v1 target remains **Agent-Gated Integration** through **Integrating**, runner-side **Local Worktree Delivery**, automated local **Squash Merge**, recorded **Integration Outcome**, and cleanup of local delivery artifacts after success.

Use `docs/MANUAL_DOGFOOD_MERGE.md` for the temporary checklist, and prefer the ordinary Agent-Gated Integration lane whenever it is available.

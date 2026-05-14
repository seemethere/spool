# Use Review Policy and agent-mediated Review Decisions

Spool will not provide the primary human review interface in v1. Humans approve or request changes in external channels such as local review, diff review, or chat, and a Review Agent records the resulting Review Decision in Spool by moving the Task from Human Review to Rework or Integrating; this preserves the non-human-facing Spool boundary while giving review states a concrete exit path.

Local-first queues default to Agent-Gated Integration: when structured acceptance and validation gates pass, the Worker Agent may move work from In Progress or Rework directly to Integrating. If the same Agent Run still owns the claim lease, it performs delivery immediately, records the Integration Outcome, and moves the Task to Done or Rework as appropriate.

Spool's own dogfood Task Queue follows this local-first default. Ordinary Spool development Tasks should not enter Human Review merely because extra confidence is useful; Worker Agents should use the advisory Subagent Review Loop before committing or requesting Integrating when risk warrants it. That advisory loop is outside Spool's Review Agent domain model and does not record Review Decisions.

Human Review is used when a queue/task Review Policy requires it or when an agent explicitly requests review; when Human Review is used, the Review Agent may prepare a Review Packet but records an actual human decision.

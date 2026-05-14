# Use built-in role prompts with repo-owned overrides

Spool v1 will ship built-in Role Prompts for Delegating Agents, Worker Agents, and Review Agents, while allowing the Managed Source Repository to override them under `.spool/prompts/delegate.md`, `.spool/prompts/worker.md`, and `.spool/prompts/review.md`. This makes the local loop usable out of the box while letting workflow instructions evolve with the codebase; runtime settings stay in Task Queue configuration rather than prompt files.

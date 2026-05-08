# Use built-in role prompts with repo-owned overrides

Tasker v1 will ship built-in Role Prompts for Delegating Agents, Worker Agents, and Review Agents, while allowing the Managed Source Repository to override them under `.tasker/prompts/delegate.md`, `.tasker/prompts/worker.md`, and `.tasker/prompts/review.md`. This makes the local loop usable out of the box while letting workflow instructions evolve with the codebase; runtime settings stay in Task Queue configuration rather than prompt files.

# Use `tasker delegate` for agent-mediated task intake

Tasker v1 will provide a `tasker delegate` entry point that launches a Pi-backed Delegation Session rather than asking humans to fill out task fields directly. The Delegating Agent uses a grill-with-docs-style Delegation Interview to clarify intent one question at a time, then creates a Root Task with a Task Brief, Acceptance Criteria, Validation Items, Priority, and initial state.

Delegation is documentation-aware but does not edit repository docs by default. It may read context and ADR files to sharpen language, then turn needed doc changes into Acceptance Criteria or Child Tasks so source changes still happen through the normal worktree/review/integration loop.

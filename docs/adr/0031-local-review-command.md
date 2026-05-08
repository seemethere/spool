# Use `tasker review` for local Human Review decisions

Tasker v1 will provide `tasker review <task_identifier>` as the local entry point for Human Review. It launches a Pi-backed Review Agent that reads the Task, Workpad Note, structured completion evidence, Task Links, and Local Worktree diff, prepares a Review Packet, asks the human for an explicit approve/rework decision through the question UI, and records the Review Decision in Tasker without requiring GitHub or a web UI.

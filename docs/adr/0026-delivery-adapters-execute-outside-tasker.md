# Execute delivery operations outside Tasker

Tasker will store Delivery Backend configuration, Delivery Records, Task Links, and Integration Outcomes, but it will not perform filesystem or Git delivery operations itself. A Symphony-side Delivery Adapter executes Local Worktree Delivery operations such as creating worktrees and performing local merges, preserving Tasker as the task source of truth rather than turning it into a repository command runner.

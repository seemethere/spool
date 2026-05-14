# Include a thin Symphony Integration for the local loop

Spool v1 will include the minimal runner-side Symphony Integration needed to validate the local workflow end-to-end, not just the HTTP task service. The Spool Service owns task state, leases, workpad data, and delivery records; the adapter claims Tasks, prepares Local Worktrees, runs agents, records work updates, and performs Integrating through Local Worktree Delivery, while staying thin and separable from the core service.

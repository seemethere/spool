# Make v1 local-first with per-Task worktrees

Spool v1 will optimize for a local developer workflow: agents work in per-Task Git worktrees, and approved work is integrated by merging the worktree branch into the local main branch. This deliberately avoids a GitHub or pull-request dependency while validating whether Spool speeds up day-to-day development.

The integration mechanism will be modeled as a pluggable Delivery Backend. Local Worktree Delivery is the only v1 backend, but the boundary should allow later pull-request, patch-file, or remote-review delivery backends without changing the core Task, Task State, or Agent Run model.

Spool records delivery configuration, delivery records, Task Links, and integration outcomes. A Symphony-side Delivery Adapter performs the filesystem and Git operations so Spool does not become the component that runs arbitrary repo commands.

The v1 Local Worktree Delivery configuration is intentionally small: managed source repository path, main branch, worktree root, branch template, and an optional done-worktree retention flag. Setup and validation commands stay in the agent workflow rather than becoming Spool build configuration.

The configured repository is a Managed Source Repository: by choosing Local Worktree Delivery, the operator opts into Spool/Symphony mutating its Main Branch, creating/removing worktrees, and cleaning task branches. Spool should warn clearly at queue setup/startup, and unexpected uncommitted changes in that repository are treated as operational failure rather than ad-hoc user work to preserve.

Before local integration, the Delivery Adapter requires a clean Local Worktree with work committed on the Task Branch, then checks that the Task Branch includes the current Main Branch commit or that the Task's Validated Base Commit still equals Main Branch. If Main Branch moved after validation or the worktree has uncommitted changes, integration fails as work-change failure so the agent can fix the branch and rerun validation.

Rework continues from the existing Local Worktree and Task Branch by default, because local iterative revision is cheap and useful. Reset Rework is explicit when the current attempt should be discarded and restarted from Main Branch.

Local Worktree Delivery defaults to a squash merge: a Task Branch may contain multiple Task Commits, but Main Branch receives one Final Commit per completed Task. The Final Commit uses the Managed Source Repository's configured Git identity and includes Spool metadata such as Task Identifier, title, and optionally run ID in the commit message. A queue-level merge strategy override can be added later if preserving detailed commit history becomes important.

After successful integration, the Delivery Adapter removes the Local Worktree and deletes the Task Branch once the work is merged into the Main Branch, while Spool keeps the delivery record, integration outcome, final commit SHA, and audit events. Queues may retain done worktrees for debugging.

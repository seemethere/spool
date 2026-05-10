# Use Integrating as the backend-neutral delivery state

Tasker will use Integrating, not Merging, as the Task State for work being delivered by a queue's configured Delivery Backend. In v1, Local Worktree Delivery implements this with a local merge into the Main Branch, but the lifecycle term stays independent of GitHub, pull requests, or any future delivery backend.

Every integration attempt records an Integration Outcome. Work-change failures such as merge conflicts, stale branches, stale validation base, or validation failures move the Task from Integrating to Rework; retryable operational failures such as dirty managed repositories, repository locks, or transient local filesystem/tooling errors leave it in Integrating with structured retry metadata and bounded backoff before operator intervention is required. Tasks with no repository changes still pass through Integrating with a `no_changes` outcome before moving to Done, avoiding a separate delivery-required flag in v1.

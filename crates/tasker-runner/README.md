# tasker-runner

`tasker-runner` owns Tasker runner-side workflow behavior. Keep `tasker-cli` focused on command parsing, facade wiring, and operator-facing output; move Worker Loop, Agent Launcher, supervisor, Local Worktree Delivery, commit metadata, and Managed Source Repository operation-lock behavior tests here with the behavior they cover.

## Targeted validation commands

Use the narrowest command that covers the touched runner area before running broader checks:

- Operation locks: `cargo test -p tasker-runner repo_lock`
- Local Worktree Delivery and Final Commit metadata: `cargo test -p tasker-runner local_worktree_delivery commit_metadata`
- Worker Loop, fake/pi Agent Launcher, prompt, and Local Worktree setup: `cargo test -p tasker-runner worker::tests`
- Supervisor orchestration and supervisor locks: `cargo test -p tasker-runner supervisor::tests`
- Full runner crate: `cargo test -p tasker-runner`

For CLI facade changes that call runner APIs, pair the runner command with a focused `tasker-cli` test filter for the command UX or output formatting you touched. End runner-extraction cleanup with `cargo fmt --check` and `cargo test --workspace`.

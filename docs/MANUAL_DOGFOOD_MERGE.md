# Manual Dogfood Merge

Manual Dogfood Merge is a temporary dogfooding escape hatch until automatic Integrating is implemented. Tasker records Tasks, Agent Runs, Task Links, Workpad Notes, and Launcher Session Data; the operator performs final Git inspection and merge outside the Tasker Service.

## Inspect the Task and Agent Run

```bash
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data task show <task-identifier>
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data run show <agent-run-id>
```

Check:

- Task State and requirements
- Workpad Note handoff/evidence
- Local Worktree Task Link
- Task Branch Task Link
- Run Transcript path and failure reason, if any

## Validate in the Local Worktree

From the Local Worktree path recorded on the Task:

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Run extension checks when TypeScript extension files changed:

```bash
cd extensions/tasker-pi
bun test
bun run build
```

Commit focused Task Branch changes if the Worker Agent left uncommitted files.

## Merge manually

From the Managed Source Repository, inspect the Task Branch against the Main Branch and perform the local merge strategy chosen by the operator. Tasker does not perform Git mutations in the Tasker Service.

After merge and validation, record a final Workpad Note or audit-relevant context through the CLI/API, then request Task State transitions only through supported Tasker gates.

This procedure is intentionally temporary and does not replace the target Integrating implementation.

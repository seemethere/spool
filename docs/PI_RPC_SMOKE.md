# Opt-in Pi RPC smoke test

This is an opt-in local check for Dogfooding Readiness. It is not part of deterministic CI because it requires a local `pi` installation, model credentials, and a running Tasker Service.

## Prerequisites

1. Build and test deterministic paths first:
   ```bash
   cargo test
   cargo clippy --all-targets --all-features -- -D warnings
   cd extensions/tasker-pi && bun test && bun run build
   ```
2. Start the Tasker Service in one terminal:
   ```bash
   cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data serve
   ```
3. Ensure a Ready Task exists in the dogfood Task Queue:
   ```bash
   cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data status
   ```
4. Ensure `pi` can load the Tasker Pi Extension from `extensions/tasker-pi` in your local pi setup.

## Smoke command

Run one Worker Loop iteration with a fresh Pi RPC process:

```bash
cargo run -p tasker-cli -- \
  --config .tasker/config.toml \
  --data-dir .tasker/data \
  work --once --queue TASKER --launcher pi \
  --api-url http://127.0.0.1:4317
```

The Worker Loop claims one Task, prepares a Local Worktree, starts `pi --mode rpc`, and exports Tasker extension environment variables including `TASKER_API_URL`, `TASKER_API_TOKEN`, and `TASKER_AGENT_RUN_ID`.

## Inspect results

Use the printed Agent Run ID:

```bash
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data run show <agent-run-id>
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data task show <task-identifier>
```

The Run Transcript is stored under `.tasker/data/runs/<agent-run-id>/`. If pi exits nonzero or emits an unattended question/confirmation event, Tasker records a failed Agent Run with a clear failure reason.

## First Dogfood Run Notes

The first real Pi Worker Loop attempt successfully exercised the unattended Tasker Pi Extension path, but it also exposed launcher bugs before a successful Agent Run was recorded. After each dogfood run, operators should inspect `tasker run show <agent-run-id>` and the saved Run Transcript under `.tasker/data/runs/<agent-run-id>/` before trusting the handoff.

Keep these notes focused on Dogfooding Readiness observability: confirming the Agent Run outcome, launcher session metadata, failure reason when present, and transcript location. Do not treat this smoke path as a broader product workflow or as a replacement for the structured Tasker gates.

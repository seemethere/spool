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
  --api-url http://127.0.0.1:4317 \
  --max-run-seconds 1800
```

The Worker Loop claims one Task, prepares a Local Worktree, starts `pi --mode rpc`, and exports Tasker extension environment variables including `TASKER_API_URL`, `TASKER_API_TOKEN`, and `TASKER_AGENT_RUN_ID`. The Pi Launcher also intentionally sets `CARGO_TARGET_DIR` for the Worker Agent process to `.tasker/data/cargo-target/<repo-name>-<path-hash>/`, a Tasker-managed shared build directory keyed by the Managed Source Repository path. This overrides any caller-provided `CARGO_TARGET_DIR` so dogfood Worker Agent worktrees do not each accumulate their own `target/` tree; operators may delete the shared directory when reclaiming space.

## Inspect results

Use the printed Agent Run ID:

```bash
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data run show <agent-run-id>
cargo run -p tasker-cli -- --config .tasker/config.toml --data-dir .tasker/data task show <task-identifier>
```

The Run Transcript is stored under the active Tasker data directory at `runs/<agent-run-id>/`; for project dogfooding through `bin/tasker-local` this resolves to `.tasker/data/runs/<agent-run-id>/`, not the default user data directory. Recorded transcript paths are absolute so `tasker run show`, monitor output, and cleanup inspection can locate artifacts reliably. The Pi Launcher treats stdout as JSONL RPC events: fire-and-forget extension UI requests such as `notify` are safe to ignore, while blocking `select`, `confirm`, `input`, or `editor` extension UI requests fail the unattended Agent Run with a clear failure reason. Supplying `--max-run-seconds` bounds launcher execution; if the duration elapses before an `agent_end` event, the Agent Run fails with a timeout reason while keeping Run Transcript and Launcher Session Data for inspection. Operators can inspect reclaimable artifact space with `tasker cleanup runs` and must pass `--delete` before saved transcript/session artifact files are removed; database rows for Tasks, Agent Runs, Launcher Session Data, and Audit Events remain authoritative.

## First Dogfood Run Notes

The first real Pi Worker Loop attempt successfully exercised the unattended Tasker Pi Extension path, but it also exposed launcher bugs before a successful Agent Run was recorded. After each dogfood run, operators should inspect `tasker run show <agent-run-id>` and the saved Run Transcript under the active data directory, usually `.tasker/data/runs/<agent-run-id>/` for project dogfooding, before trusting the handoff.

Keep these notes focused on Dogfooding Readiness observability: confirming the Agent Run outcome, launcher session metadata, failure reason when present, and transcript location. Do not treat this smoke path as a broader product workflow or as a replacement for the structured Tasker gates.

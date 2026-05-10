# Tasker monitor smoke checklist

`tasker monitor` is a read-only attention-first status board for operator observability. Interactive mode uses ratatui on top of raw terminal mode plus the alternate screen; plain mode prints a normal snapshot without terminal control sequences. The default board prioritizes Needs Attention, Running, and a small Next list rather than a full queue dashboard.

## Plain fallback

Use this when terminal capabilities are limited, when capturing output, or when validating the fallback path:

```bash
cargo run -p tasker-cli -- \
  --config /Users/eliuriegas/projects/tasker/.tasker/config.toml \
  --data-dir /Users/eliuriegas/projects/tasker/.tasker/data \
  monitor --queue TASKER --once --plain
```

Equivalent installed-binary form:

```bash
tasker monitor --queue TASKER --once --plain
```

If stdout is not an interactive terminal or `TERM=dumb`, `tasker monitor` falls back to one plain snapshot and warns that stdout is not an interactive terminal. The ratatui path is intentionally read-only and focuses on operator attention items first: stale or failed Agent Runs, active Retry Holds, Integrating Tasks waiting for progress, integration retry waits, Managed Source Repository operation locks, compact healthy Agent Runs, a limited Ready Task Next list, recent Agent Run outcomes, and active config/database context. Use `tasker status`, `tasker task show`, or `tasker run show` for fuller queue and run details.

## Remote terminal and tmux smoke

Run this opt-in smoke from the operator environment Tasker is used in, especially over the current remote connection inside tmux:

1. Ensure the Tasker Service is available for the selected config/database.
2. In tmux over the remote connection, run:
   ```bash
   cargo run -p tasker-cli -- \
     --config /Users/eliuriegas/projects/tasker/.tasker/config.toml \
     --data-dir /Users/eliuriegas/projects/tasker/.tasker/data \
     monitor --queue TASKER --refresh-seconds 1
   ```
3. Confirm the screen refreshes in place, lines start at column 0 without staircase indentation, and the board is ordered as Needs Attention, Running, then Next/Recent.
4. Press `r` to refresh, then `q`, `Esc`, or `Ctrl-C` to quit.
5. If rendering is not trustworthy, rerun the documented plain fallback command above and capture the terminal type with `echo "$TERM"`.

This smoke is not deterministic CI coverage; it validates ratatui/raw-mode rendering in the real operator terminal. Deterministic coverage lives in the `tasker-cli` monitor tests, including attention-first ordering, compact title truncation, attention row generation, a ratatui `TestBackend` render check, and raw-mode newline normalization for the plain renderer.

# Tasker monitor smoke checklist

`tasker monitor` is a read-only terminal status monitor for operator observability. Interactive mode uses raw terminal mode plus the alternate screen; plain mode prints a normal snapshot without terminal control sequences.

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

If stdout is not an interactive terminal or `TERM=dumb`, `tasker monitor` falls back to one plain snapshot and warns that stdout is not an interactive terminal.

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
3. Confirm the screen refreshes in place and lines start at column 0 without staircase indentation.
4. Press `r` to refresh, then `q`, `Esc`, or `Ctrl-C` to quit.
5. If rendering is not trustworthy, rerun the documented plain fallback command above and capture the terminal type with `echo "$TERM"`.

This smoke is not deterministic CI coverage; it validates raw-mode rendering in the real operator terminal. Deterministic coverage lives in the `tasker-cli` monitor tests, including raw-mode newline normalization.

# Repository-local Spool state hygiene

This guide explains the local `.spool/` files created when an existing repository adopts Spool with repository-local config:

```bash
spool init --config .spool/config.toml --data-dir .spool/data
```

Spool remains local-first: these files are not uploaded or shared by Spool automatically. Treat the active Spool data directory as local Operator state for the repository unless your team has a deliberate policy for exporting sanitized examples.

## What lives under `.spool/`

| Path or artifact | What it is | Hygiene guidance |
| --- | --- | --- |
| `.spool/config.toml` | Repository-local Spool config. When this config is active and no explicit data override is passed, Spool resolves the data directory to `.spool/data/` beside the config. | Local Operator configuration. Do not commit by default; it may encode local paths, bind settings, or database location choices. Commit only a sanitized example such as `.spool/config.example.toml` if your repository wants one. |
| `.spool/data/spool.db` | SQLite **Task Backend** database containing authoritative Spool records: **Task Queues**, **Tasks**, requirement statuses, **Workpad Notes**, **Agent Runs**, **Delivery Records**, **Launcher Session Data** rows, **Audit Events**, and API tokens. | Sensitive, local-only, and authoritative. Do not commit. Do not delete as routine cleanup; back it up or migrate it intentionally if you need to move local Spool state. |
| API tokens | Bearer tokens created by `spool init` / `spool db migrate` and stored in the local Task Backend. The token is printed so the Operator can configure `spool serve` and the **Spool Pi Extension**. | Secret local credentials. Do not commit tokens, paste them into docs, or include them in Run Transcript excerpts. Rotate/recreate through explicit Operator paths rather than editing files by hand. |
| `.spool/data/runs/<agent_run_id>/` | Saved **Run Transcript** files and raw launcher artifacts for an **Agent Run**. | Local debugging artifacts that may contain prompts, tool arguments, paths, command output, or secrets. Do not commit or share raw bodies. Use `spool run show` for metadata and `spool cleanup runs` dry-runs before deleting artifact files. |
| **Launcher Session Data** | Normalized launcher metadata stored in the database, plus optional raw launcher/session artifacts under `runs/`. | Local by default and never automatically uploaded. Numeric summaries can be useful for local workflow metrics; raw payloads should stay out of commits, docs, and telemetry exports unless explicitly sanitized. |
| `.spool/worktrees/` or your configured **Worktree Root** | Parent directory for per-Task **Local Worktrees** created by **Local Worktree Delivery**. | Local delivery workspace. Do not commit. Active **Local Worktrees** and **Task Branches** are working state; remove only through integration, cancellation cleanup, **Reset Rework**, or `spool cleanup local-worktrees` after inspecting the dry-run report. |
| `.spool/data/cargo-target/` and per-worktree `target/` directories | Rebuildable Rust build output used during dogfooding/Worker Agent runs. | Regenerable local cache. Do not commit. Reclaim with `spool cleanup cargo-targets` or ordinary build-cache cleanup after confirming the path. |
| `.spool/prompts/*.md` | Optional repo-owned **Prompt Overrides** for built-in Role Prompts. | These are source-controlled workflow instructions if your repository deliberately uses them. Keep them free of secrets and local machine paths. |
| `.spool/bootstrap-tasks/TEMPLATE.md`, `.spool/validation-commands.txt`, and similar curated files | Repository-owned examples or validation configuration intentionally kept with source. | Safe to commit only when reviewed as normal repository content. They should not include tokens, local database paths, raw transcripts, or private machine-specific state. |

## Authoritative records vs local artifacts

Current database rows are authoritative for Spool state. **Audit Events** are append-only history, and current relational rows remain the read model for **Tasks**, requirements, **Agent Runs**, **Delivery Records**, **Integration Outcomes**, API tokens, and **Launcher Session Data** metadata.

Local files such as **Run Transcripts**, raw launcher payloads, build outputs, and completed **Local Worktrees** are storage artifacts around that authoritative state. They can still be sensitive and useful for diagnosis, so Spool does not delete them during ordinary **Worker Loop** execution. Cleanup is an explicit **Operator** action:

```bash
spool cleanup runs --config .spool/config.toml --data-dir .spool/data
spool cleanup local-worktrees --queue APP --config .spool/config.toml --data-dir .spool/data
spool cleanup cargo-targets --queue APP --config .spool/config.toml --data-dir .spool/data
```

These commands default to dry-run reporting. Pass `--delete` only after the report identifies the expected safe cleanup candidates. Run artifact cleanup removes saved transcript/session artifact files, not authoritative Task, Agent Run, Launcher Session Data database rows, or Audit Events.

## Suggested `.gitignore`

A conservative default is to ignore all repository-local Spool state and then opt in only specific reviewed files that are meant to be source-controlled:

```gitignore
# Spool local operator state: config, SQLite database, API tokens,
# Run Transcripts, Launcher Session Data artifacts, Local Worktrees, and caches.
/.spool/*

# Optional: commit only curated, secret-free repository-owned Spool files.
!/.spool/bootstrap-tasks/
/.spool/bootstrap-tasks/*
!/.spool/bootstrap-tasks/TEMPLATE.md
!/.spool/validation-commands.txt
# !/.spool/prompts/
# !/.spool/prompts/*.md
```

If you choose to version **Prompt Overrides** under `.spool/prompts/`, review them like source code: no API tokens, no private absolute paths, no raw prompt/session dumps, and no local Task Backend data.

## Path safety reminders

- Use explicit `--config .spool/config.toml --data-dir .spool/data` until your shell reliably targets the intended repository-local Task Backend.
- If a repository-local `.spool/config.toml` exists but is not the active config, Spool warns for read-only commands and refuses unsafe mutating commands unless the Operator explicitly selects a config or data/database override.
- Do not use a **Local Worktree** or **Task Branch** to migrate the shared project database with unintegrated code; `spool db migrate` is an explicit Operator path intended for the trusted **Managed Source Repository** **Main Branch** by default.
- Do not paste raw **Run Transcripts**, raw **Launcher Session Data**, prompt bodies, tool arguments, API tokens, or large logs into commits, docs, Workpad handoffs, or external telemetry.

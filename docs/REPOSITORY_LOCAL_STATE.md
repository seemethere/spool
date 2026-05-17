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

## Pre-publication checklist

Before publishing a Spool repository publicly, the **Operator** should do one final local hygiene pass. This is repository release guidance, not a new Spool workflow, and it does not require GitHub, Linear, pull requests, or hosted services.

1. Inspect the source tree and ignored files before packaging or pushing:

   ```bash
   git status --short --ignored
   git ls-files --others --ignored --exclude-standard
   git diff --check
   git log --oneline --decorate --max-count=20
   ```

   Confirm that only intentional source files are tracked. Do not publish `.spool/`, historical `.tasker/` state, `.spool/data/spool.db`, other local databases, API tokens, **Run Transcript** directories, raw **Launcher Session Data** artifacts, `.spool/worktrees/`, Task Branch scratch state, `target/`, `.spool/data/cargo-target/`, per-worktree build outputs, or `node_modules/`.

2. Run the existing Spool cleanup dry-runs for repository-local artifacts, then decide whether to delete only verified-safe candidates:

   ```bash
   spool cleanup runs --config .spool/config.toml --data-dir .spool/data
   spool cleanup local-worktrees --queue APP --config .spool/config.toml --data-dir .spool/data
   spool cleanup cargo-targets --queue APP --config .spool/config.toml --data-dir .spool/data
   ```

   These are Operator cleanup commands. They default to dry-run reporting; pass `--delete` only after reviewing the reported paths.

3. Check for stale rename residue, local paths, and accidentally documented secrets before release:

   ```bash
   rg -n "tasker|\.tasker|extensions/tasker-pi|spool\.db|api[_-]?token|SPOOL_API_TOKEN|BEGIN (RSA|OPENSSH|PRIVATE) KEY" README.md docs crates extensions scripts .gitignore
   rg -n "github.com[:/]seemethere/tasker|/Users/|\.spool/data|runs/[0-9a-f-]+" README.md docs crates extensions scripts .gitignore
   ```

   Any remaining matches should be intentional historical migration context, local-state warnings, or sanitized examples. Do not paste raw transcript bodies or launcher payloads into the repository to explain a release issue.

4. Verify package and extension metadata without relying on hosted services:

   ```bash
   cargo metadata --format-version 1 --no-deps
   bun install --cwd extensions/spool-pi
   bun run --cwd extensions/spool-pi build
   bun test --cwd extensions/spool-pi
   ```

   Review `extensions/spool-pi/package.json`, its `files` list, license, repository metadata, and README so package setup matches the public source tree and does not include local-only artifacts.

5. Run the repository's normal deterministic validation from a clean checkout or clean **Managed Source Repository**:

   ```bash
   cargo fmt --all -- --check
   cargo test --workspace
   ```

   If you keep a curated `.spool/validation-commands.txt`, run those commands too. Keep validation evidence as concise command results; do not commit local Task Backend data, Run Transcripts, raw logs, or secrets as proof.

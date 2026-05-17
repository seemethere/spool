# Contributing to Spool

Thanks for helping improve Spool. This repository is still early v1 software, so contributions should keep the scope local-first, CLI-first, and focused on the **Task Backend** needed for agent-driven development.

## Development expectations

- Use the domain language in [`CONTEXT.md`](CONTEXT.md).
- Check [`ROADMAP.md`](ROADMAP.md) before broad feature work.
- Read relevant ADRs in [`docs/adr/`](docs/adr/) before changing architecture, persistence, workflow, delivery, launcher behavior, or public terminology.
- Keep changes focused and deterministic. Prefer small slices with matching tests or documentation updates.
- Do not introduce a GitHub-required, pull-request-required, hosted-service, web UI, external tracker sync, or generic project management workflow into v1 scope.

## Local validation

Run the main workspace checks before handing off changes:

```bash
cargo fmt --all -- --check
cargo test --workspace
```

If a full workspace test is not practical for a change, run the narrow deterministic check that covers the change and document the reason.

## Repository-local Spool state

Spool stores local Operator state such as config, SQLite data, API tokens, Run Transcripts, Launcher Session Data, Local Worktrees, and delivery artifacts. Treat that data as local by default. Do not commit secrets, raw transcripts, prompt bodies, raw launcher payloads, or unrelated queue data.

When working in this repository's dogfood setup, use `bin/spool-local` from the Managed Source Repository root for Spool CLI reads and operator/debug commands so the project Task Backend is selected explicitly.

## Documentation

Update documentation in the same change when behavior or domain meaning changes:

- `CONTEXT.md` for canonical domain language or relationships.
- ADRs only for decisions that are hard to reverse, surprising without context, and trade-off driven.
- `ROADMAP.md` when milestone sequencing changes.
- Public-facing docs when setup, validation, security, or contribution expectations change.

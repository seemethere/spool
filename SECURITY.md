# Security Policy

Spool is early local-first software. There is no hosted Spool service and no guaranteed support SLA.

## Reporting a vulnerability

Please do not publish exploit details, secrets, raw Run Transcripts, prompt bodies, API tokens, or local Operator data in a public issue.

If GitHub private vulnerability reporting is enabled for the public repository, use that path. Otherwise, contact a maintainer through a private channel if one is available from the repository or package metadata, and include only the minimum information needed to reproduce the issue safely.

Useful reports include:

- affected commit or version;
- concise impact summary;
- local reproduction steps with sanitized data;
- whether local files, API tokens, Run Transcripts, Launcher Session Data, Local Worktrees, or Managed Source Repository contents may be exposed or mutated unexpectedly.

## Scope

Security-sensitive areas include the Spool API, local SQLite persistence, API tokens, repository-local `.spool/` state, Local Worktree Delivery, Agent Launchers, the Pi Launcher, and the Spool Pi Extension.

Spool's default threat model is local-first development infrastructure. Operators remain responsible for protecting their machines, repositories, shell environments, and model/provider credentials.

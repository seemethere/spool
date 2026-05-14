# Extract runner-side workflow into spool-runner

Spool will extract runner-side execution and delivery behavior from `spool-cli` into a new Rust crate named `spool-runner`. `spool-cli` remains a thin command facade, while `spool-runner` owns Worker Loop orchestration, supervisor orchestration, Agent Launcher handling, Local Worktree setup and integration, and Managed Source Repository operation-lock mechanics. The first extraction is behavior-preserving and may keep direct `spool-db` calls as a transitional constraint; implementation sequencing and the later Spool API boundary are tracked in `docs/RUNNER_EXTRACTION_PLAN.md`.


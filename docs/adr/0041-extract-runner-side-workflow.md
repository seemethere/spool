# Extract runner-side workflow into tasker-runner

Tasker will extract runner-side execution and delivery behavior from `tasker-cli` into a new Rust crate named `tasker-runner`. `tasker-cli` remains a thin command facade, while `tasker-runner` owns Worker Loop orchestration, supervisor orchestration, Agent Launcher handling, Local Worktree setup and integration, and Managed Source Repository operation-lock mechanics. The first extraction is behavior-preserving and may keep direct `tasker-db` calls as a transitional constraint; implementation sequencing and the later Tasker API boundary are tracked in `docs/RUNNER_EXTRACTION_PLAN.md`.


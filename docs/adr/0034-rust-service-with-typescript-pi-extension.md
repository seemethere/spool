# Use Rust for Tasker and TypeScript for the pi extension

Tasker v1 will implement the service, CLI, worker loop, and SQLite persistence in Rust, while the Tasker Pi Extension will be a small TypeScript package because pi extensions are TypeScript modules. The two parts communicate through the Tasker HTTP API rather than sharing in-process code, keeping the core backend independent from pi's extension runtime.

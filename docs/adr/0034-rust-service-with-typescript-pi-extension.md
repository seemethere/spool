# Use Rust for Spool and TypeScript for the pi extension

Spool v1 will implement the service, CLI, worker loop, and SQLite persistence in Rust, while the Spool Pi Extension will be a small TypeScript package because pi extensions are TypeScript modules. The two parts communicate through the Spool HTTP API rather than sharing in-process code, keeping the core backend independent from pi's extension runtime.

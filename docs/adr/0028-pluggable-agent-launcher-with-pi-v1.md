# Use a pluggable Agent Launcher with pi for v1

The runner-side Symphony Integration will use a pluggable Agent Launcher boundary rather than baking one coding-agent protocol into Spool. Spool records Agent Runs, outcomes, and optional launcher metadata, while the adapter owns protocol-specific execution; the concrete v1 launcher will run Worker Agents through pi so the local workflow can be validated with the current development harness.

Pi Launcher will use `pi --mode rpc` over JSONL stdin/stdout. RPC mode is language-neutral for the Rust adapter and supports streaming events, aborts, session control, and extension UI requests; one-shot print/JSON mode is too limited, and the TypeScript SDK would couple the adapter to Node.js.

Each Agent Run starts a fresh Pi RPC Session and may save a Run Transcript for debugging. Continuity across retries comes from the Local Worktree, Workpad Note, structured Task data, and Audit Events rather than hidden resumed chat history.

Question UI is allowed for interactive Delegation and Review Sessions. During unattended Worker Loops, unexpected pi extension UI requests fail the Agent Run with a clear reason and the structured `unattended_question` failure reason code instead of stalling for human input. Pi Launcher startup, RPC I/O, process exit, and timeout failures likewise record stable Agent Run failure reason codes for operator recovery and workflow metrics.

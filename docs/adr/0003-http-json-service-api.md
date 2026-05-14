# Make Spool an HTTP+JSON service

Spool will expose a small HTTP+JSON API backed by SQLite as the canonical v1 integration surface, with an optional thin CLI for operator convenience. This keeps Symphony decoupled from Spool's implementation language and runtime, while avoiding the tighter coupling of an embedded Rust library or the awkwardness of making a CLI the primary programmatic interface.

The v1 API will be resource-oriented for normal reads/updates and use explicit command endpoints for domain actions such as claim-next, transition, heartbeat, finish-run, and repair-override. Spool will avoid GraphQL and JSON-RPC in v1 because the domain is small and action-heavy.

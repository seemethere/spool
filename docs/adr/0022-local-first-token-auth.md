# Use local-first bearer-token API auth

Spool v1 will bind to `127.0.0.1` by default and require bearer-token authentication for mutating APIs plus claim/run APIs, while leaving health/version endpoints unauthenticated. Binding beyond localhost requires explicit configuration and a token; this avoids accidentally exposing agent task control while deferring mTLS and multi-tenant authorization.

The authorization model is a single trusted workspace with one or more API tokens for rotation and integrations. Spool v1 will not model users, tenants, per-queue permissions, or ACLs.

# Require actor attribution for Tasker mutations

Tasker will record a first-class Actor on mutating API calls, including Task creation, state transitions, workpad edits, review decisions, and run heartbeats. Distinguishing Delegating Agents, Worker Agents, Review Agents, Operators, and system-generated events makes delegation, audit history, and repair actions understandable instead of leaving important workflow changes anonymous.

Bearer tokens authenticate API clients, but they do not replace Actor attribution. A caller supplies the Actor explicitly, Tasker validates that actor kind is allowed for the endpoint, and Audit Events record the Actor that caused the domain change.

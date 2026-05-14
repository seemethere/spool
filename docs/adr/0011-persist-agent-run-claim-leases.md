# Persist Agent Run claim leases in Spool

Spool will persist minimal Agent Run records as claim leases, while Symphony remains responsible for launching and managing agent processes. The API will support claiming the next eligible Task, heartbeating the Run, and finishing the Run; this prevents duplicate pickup across workers and gives recovery visibility without turning Spool into an orchestrator.

Claiming a Ready Task will atomically create the Agent Run and transition the Task to In Progress. Claiming In Progress, Rework, or Integrating creates the Agent Run without changing state so agents can still route based on workflow meaning.

When a lease expires without a normal finish signal, Spool will mark the Agent Run expired with a structured `claim_lease_expired` failure reason code and make the Task eligible for another claim if its current state is agent-eligible and it is not blocked. Lease expiry does not roll the Task back to Ready because execution failure is not a lifecycle reversal.

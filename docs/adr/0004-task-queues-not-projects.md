# Use Task Queues instead of Projects for v1 work selection

Tasker will use a lightweight Task Queue as the grouping key that Symphony workflows poll for work, and each Task will belong to exactly one Task Queue. This provides the routing/filtering role currently served by Linear project slugs without importing Linear's broader Project semantics such as teams, cycles, milestones, permissions, or planning workflows.

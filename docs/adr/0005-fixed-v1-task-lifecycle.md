# Use a fixed v1 Task State lifecycle

Spool v1 will use a fixed Task State lifecycle: Backlog, Ready, In Progress, Human Review, Rework, Integrating, Done, and Canceled. Arbitrary queue-defined workflows would require state-category mapping and workflow-engine behavior before the core agent loop is proven, so custom workflows are deferred until after the minimal Symphony-compatible backend is working.

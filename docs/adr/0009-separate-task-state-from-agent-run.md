# Separate Task State from Agent Run

Tasker will treat In Progress as a task lifecycle state, not as evidence that an agent process is currently running. Live execution attempts are modeled separately as Agent Runs, which keeps retries, restarts, and delegated blocking work from corrupting the meaning of the Task State.

Finishing an Agent Run records an execution outcome and releases the claim lease, but it does not directly change Task State. Task State changes happen through explicit transition APIs, except for the atomic Ready → In Progress transition performed when a Task is claimed.

# Expose Tasker workflow updates through a pi extension

Tasker v1 will ship a Tasker Pi Extension that exposes narrow, typed workflow tools to Worker Agents, while keeping a CLI for operator and debugging use. Agents should use extension tools to read Task context, update the Workpad Note, mark criteria and validation statuses, create Child or Follow-up Tasks, attach Task Links, and request State Transitions instead of shelling out to broad `tasker` CLI commands for core workflow updates.

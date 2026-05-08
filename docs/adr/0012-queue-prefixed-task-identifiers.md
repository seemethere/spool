# Use queue-prefixed Task Identifiers

Tasker will give every Task both an immutable internal UUID and a human-readable Task Identifier generated as `<TASK_QUEUE_KEY>-<sequence>`, such as `CORE-123`. Symphony needs readable, stable identifiers for prompts, logs, and workspace paths, while internal UUIDs keep API/database identity independent from human-facing keys.

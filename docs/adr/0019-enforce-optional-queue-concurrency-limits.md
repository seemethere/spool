# Enforce optional queue concurrency limits in Tasker

Tasker will support an optional Queue Concurrency Limit and enforce it during claim operations, even though Symphony may also apply a global worker limit. Keeping the queue limit in Tasker protects shared resources when multiple Symphony instances poll the same queue, while leaving the field optional avoids adding concurrency policy to simple deployments.

# Expose a first-class Tasker API instead of a Linear-compatible facade

Tasker will integrate with Symphony through a dedicated Tasker API and Symphony adapter rather than pretending to be Linear's GraphQL API. A Linear-compatible facade would reduce short-term adapter work, but it would pull Linear's domain assumptions into Tasker and undermine the decision to build a focused task backend instead of an issue-tracker clone.

The canonical API uses Tasker-native terms such as Task, Task Tag, and Blocking Task. The Symphony Adapter may map those into Symphony's existing normalized issue shape internally during integration, but Tasker will not expose Linear-shaped field names as its public contract.

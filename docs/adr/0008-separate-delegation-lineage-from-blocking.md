# Separate delegation lineage from blocking

Spool will model parent/child lineage separately from dependency blocking. A Child Task records that an agent delegated work while executing another Task, but it blocks the parent only when explicitly marked as a Blocking Task; this prevents follow-up discoveries from silently expanding the parent Task's completion scope.

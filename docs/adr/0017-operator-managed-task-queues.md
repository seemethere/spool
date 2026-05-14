# Make Task Queues operator-managed

Spool will treat Task Queues as operator-managed infrastructure boundaries rather than normal agent-created data. Agents may create Tasks inside existing queues, but queue creation, renaming, and deletion require an admin/operator path because queue keys shape Task Identifiers, workspace routing, and Symphony polling configuration.

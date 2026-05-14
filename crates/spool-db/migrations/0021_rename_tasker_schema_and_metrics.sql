ALTER TABLE tasker_metadata RENAME TO spool_metadata;

ALTER TABLE agent_run_metrics
    RENAME COLUMN repeated_tasker_context_fetch_count TO repeated_spool_context_fetch_count;

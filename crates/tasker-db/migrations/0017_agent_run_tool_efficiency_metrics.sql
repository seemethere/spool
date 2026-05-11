ALTER TABLE agent_run_metrics ADD COLUMN tool_call_counts_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE agent_run_metrics ADD COLUMN repeated_read_count INTEGER;
ALTER TABLE agent_run_metrics ADD COLUMN repeated_tasker_context_fetch_count INTEGER;
ALTER TABLE agent_run_metrics ADD COLUMN shell_command_counts_json TEXT NOT NULL DEFAULT '{}';

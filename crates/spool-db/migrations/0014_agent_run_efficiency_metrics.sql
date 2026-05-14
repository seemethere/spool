ALTER TABLE agent_run_metrics ADD COLUMN tool_call_count INTEGER;
ALTER TABLE agent_run_metrics ADD COLUMN tool_error_count INTEGER;
ALTER TABLE agent_run_metrics ADD COLUMN repeated_failed_tool_attempt_count INTEGER;
ALTER TABLE agent_run_metrics ADD COLUMN assistant_turn_count INTEGER;
ALTER TABLE agent_run_metrics ADD COLUMN user_turn_count INTEGER;
ALTER TABLE agent_run_metrics ADD COLUMN max_context_tokens INTEGER;
ALTER TABLE agent_run_metrics ADD COLUMN efficiency_hints_json TEXT NOT NULL DEFAULT '[]';

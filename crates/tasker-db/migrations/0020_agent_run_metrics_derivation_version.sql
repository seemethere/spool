ALTER TABLE agent_run_metrics ADD COLUMN derivation_version INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_agent_run_metrics_derivation_version
    ON agent_run_metrics(derivation_version);

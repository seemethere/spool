ALTER TABLE integration_outcomes ADD COLUMN retryable INTEGER NOT NULL DEFAULT 0 CHECK (retryable IN (0, 1));
ALTER TABLE integration_outcomes ADD COLUMN retry_attempt INTEGER CHECK (retry_attempt IS NULL OR retry_attempt > 0);
ALTER TABLE integration_outcomes ADD COLUMN next_retry_at TEXT;

CREATE INDEX IF NOT EXISTS idx_integration_outcomes_retry
ON integration_outcomes(task_id, retryable, next_retry_at);

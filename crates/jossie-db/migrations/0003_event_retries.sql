ALTER TABLE integration_events ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE integration_events ADD COLUMN retry_started_at TEXT;
ALTER TABLE integration_events ADD COLUMN next_attempt_at TEXT;
ALTER TABLE integration_events ADD COLUMN processing_started_at TEXT;
ALTER TABLE integration_events ADD COLUMN delivery_started_at TEXT;
CREATE INDEX idx_integration_events_retry ON integration_events(status, next_attempt_at);
